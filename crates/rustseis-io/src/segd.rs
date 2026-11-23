use rayon::prelude::*;
use rustseis_core::{Result, SeismicError, Trace, TraceHeaders, TraceType};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SegdSummary {
    pub file_number: u32,
    pub format_code: u16,
    pub revision: String,
    pub dt_us: u32,
    pub record_length_ms: u32,
    pub total_channels: usize,
    pub live_channels: usize,
    pub aux_channels: usize,
    pub n_samples: usize,
    pub line_name: Option<String>,
    pub sou_x: Option<f64>,
    pub sou_y: Option<f64>,
    pub water_depth: Option<f32>,
}

#[derive(Debug, Clone)]
struct ChannelSet {
    cs_num: u8,
    n_channels: usize,
    chan_type: u8,
    trace_hdr_ext: usize,
    mp_descaling: f32,
}

fn bcd_byte(b: u8) -> u32 {
    let hi = ((b >> 4) & 0x0F) as u32;
    let lo = (b & 0x0F) as u32;
    hi * 10 + lo
}

fn bcd_u16(bs: &[u8]) -> u32 {
    bcd_byte(bs[0]) * 100 + bcd_byte(bs[1])
}

fn parse_sercel_sps_external(external_hdr: &[u8]) -> (Option<String>, Option<i32>, Option<f32>, Option<f64>, Option<f64>) {
    let end = external_hdr.iter().position(|&b| b == 0).unwrap_or(external_hdr.len().min(256));
    let s = String::from_utf8_lossy(&external_hdr[..end]);
    let trimmed = s.trim();
    // Example: "SPS SGM3-BIO-NIAS-L22    1000 0.000000  383897.6   126379.1 19:56:05 06072018"
    if trimmed.starts_with("SPS ") {
        let toks: Vec<&str> = trimmed.split_whitespace().collect();
        if toks.len() >= 6 {
            let line = Some(toks[1].to_string());
            let shot = toks[2].parse::<i32>().ok();
            let depth = toks[3].parse::<f32>().ok();
            let sx = toks[4].parse::<f64>().ok();
            let sy = toks[5].parse::<f64>().ok();
            return (line, shot, depth, sx, sy);
        }
    }
    (None, None, None, None, None)
}

fn parse_segd_header(data: &[u8]) -> Result<(SegdSummary, Vec<ChannelSet>, usize, Option<i32>)> {
    if data.len() < 64 {
        return Err(SeismicError::InvalidFormat("SEG-D file too small (< 64 bytes)".into()));
    }
    let gh1 = &data[0..32];
    let gh2 = &data[32..64];

    let file_num_gh1 = if gh1[0] == 0xFF && gh1[1] == 0xFF {
        None
    } else {
        Some(bcd_u16(&gh1[0..2]))
    };
    let fmt_code = bcd_u16(&gh1[2..4]) as u16;
    if fmt_code != 8058 {
        return Err(SeismicError::InvalidFormat(format!(
            "Unsupported SEG-D format code {fmt_code} (currently supported: 8058 IEEE float32 demux)"
        )));
    }

    let add_gh = ((gh1[11] >> 4) & 0x0F) as usize;
    let dt_us = ((gh1[22] as u32) * 1000) / 16; // base scan interval in 1/16 ms
    let chan_sets_n = if gh1[28] == 0xFF {
        u16::from_be_bytes([gh2[3], gh2[4]]) as usize
    } else {
        bcd_byte(gh1[28]) as usize
    };
    let skew_blocks = bcd_byte(gh1[29]) as usize;
    let ext_hdr_blocks = if gh1[30] == 0xFF {
        u16::from_be_bytes([gh2[5], gh2[6]]) as usize
    } else {
        bcd_byte(gh1[30]) as usize
    };
    let external_hdr_blocks = if gh1[31] == 0xFF {
        u16::from_be_bytes([gh2[7], gh2[8]]) as usize
    } else {
        bcd_byte(gh1[31]) as usize
    };

    let ext_file_num = ((gh2[0] as u32) << 16) | ((gh2[1] as u32) << 8) | (gh2[2] as u32);
    let file_number = if ext_file_num > 0 {
        ext_file_num
    } else {
        file_num_gh1.unwrap_or(0)
    };
    let revision = format!("{}.{}", gh2[10], gh2[11]);
    let ext_rec_len = ((gh2[14] as u32) << 16) | ((gh2[15] as u32) << 8) | (gh2[16] as u32);

    let csd_offset = (1 + add_gh) * 32;
    let ext_offset = csd_offset + (chan_sets_n + skew_blocks) * 32;
    let external_offset = ext_offset + ext_hdr_blocks * 32;
    let traces_offset = external_offset + external_hdr_blocks * 32;

    if data.len() < traces_offset {
        return Err(SeismicError::InvalidFormat(format!(
            "SEG-D truncated header: file={} < header_end={}",
            data.len(),
            traces_offset
        )));
    }

    let mut channel_sets = Vec::with_capacity(chan_sets_n);
    let mut total_channels = 0usize;
    let mut live_channels = 0usize;
    let mut aux_channels = 0usize;

    for i in 0..chan_sets_n {
        let off = csd_offset + i * 32;
        let csd = &data[off..off + 32];
        let n_ch = bcd_u16(&csd[8..10]) as usize;
        if n_ch == 0 {
            continue;
        }
        let cs_num = bcd_byte(csd[1]) as u8;
        let chan_type = (csd[10] >> 4) & 0x0F;
        let trace_hdr_ext = (csd[28] & 0x0F) as usize;
        // SEG-D Rev 1/2 MP descaling exponent: byte 7 (sign + 7 bits) & byte 6 (8 fractional bits) / 1024.0
        let mp_sign = if (csd[7] & 0x80) != 0 { -1.0f64 } else { 1.0f64 };
        let mp_mag = (((csd[7] & 0x7F) as u16) << 8) | (csd[6] as u16);
        let mp_descaling = if mp_mag == 0 {
            1.0f32
        } else {
            2.0_f64.powf(mp_sign * (mp_mag as f64) / 1024.0) as f32
        };
        total_channels += n_ch;
        if chan_type == 1 {
            live_channels += n_ch;
        } else {
            aux_channels += n_ch;
        }
        channel_sets.push(ChannelSet {
            cs_num,
            n_channels: n_ch,
            chan_type,
            trace_hdr_ext,
            mp_descaling,
        });
    }

    let (line_name, sps_shot, water_depth, sou_x, sou_y) = if external_hdr_blocks > 0 {
        parse_sercel_sps_external(&data[external_offset..traces_offset])
    } else {
        (None, None, None, None, None)
    };

    // Determine sample count from first trace extension or record length
    let mut n_samples = if ext_rec_len > 0 && dt_us > 0 {
        ((ext_rec_len as u64 * 1000) / dt_us as u64) as usize + 1
    } else {
        0
    };
    if let Some(first_cs) = channel_sets.first() {
        if first_cs.trace_hdr_ext >= 1 && data.len() >= traces_offset + 52 {
            let ext1 = &data[traces_offset + 20..traces_offset + 52];
            let ns = ((ext1[7] as usize) << 16) | ((ext1[8] as usize) << 8) | (ext1[9] as usize);
            if ns > 0 {
                n_samples = ns;
            }
        }
    }

    let summary = SegdSummary {
        file_number,
        format_code: fmt_code,
        revision,
        dt_us,
        record_length_ms: ext_rec_len,
        total_channels,
        live_channels,
        aux_channels,
        n_samples,
        line_name,
        sou_x,
        sou_y,
        water_depth,
    };

    Ok((summary, channel_sets, traces_offset, sps_shot))
}

/// Inspect a single SEG-D file's header metadata without decoding all trace samples.
pub fn inspect_segd_file(path: impl AsRef<Path>) -> Result<SegdSummary> {
    let data = fs::read(path.as_ref())?;
    let (summary, _, _, _) = parse_segd_header(&data)?;
    Ok(summary)
}

/// Read all traces from a single SEG-D file (format 8058 IEEE float32 demux).
pub fn read_segd_file(path: impl AsRef<Path>, live_only: bool) -> Result<(SegdSummary, Vec<Trace>)> {
    let data = fs::read(path.as_ref())?;
    let (summary, channel_sets, mut offset, sps_shot) = parse_segd_header(&data)?;

    let ffid = sps_shot.unwrap_or(summary.file_number as i32);
    let mut traces = Vec::with_capacity(if live_only {
        summary.live_channels
    } else {
        summary.total_channels
    });

    let mut live_chan_idx = 0i32;
    let mut seq = 0i32;

    for cs in &channel_sets {
        for _ in 0..cs.n_channels {
            if offset + 20 > data.len() {
                return Err(SeismicError::InvalidFormat("Unexpected EOF in SEG-D trace header".into()));
            }
            let th = &data[offset..offset + 20];
            let chan_bcd = bcd_u16(&th[4..6]) as i32;
            let th_ext = (th[9] as usize).max(cs.trace_hdr_ext);
            let ext_bytes = th_ext * 32;
            if offset + 20 + ext_bytes > data.len() {
                return Err(SeismicError::InvalidFormat("Unexpected EOF in SEG-D trace ext header".into()));
            }
            let ns = if th_ext >= 1 {
                let ext1 = &data[offset + 20..offset + 52];
                let s = ((ext1[7] as usize) << 16) | ((ext1[8] as usize) << 8) | (ext1[9] as usize);
                if s > 0 { s } else { summary.n_samples }
            } else {
                summary.n_samples
            };

            let data_start = offset + 20 + ext_bytes;
            let data_end = data_start + ns * 4;
            if data_end > data.len() {
                return Err(SeismicError::InvalidFormat(format!(
                    "Unexpected EOF in SEG-D trace data: need {data_end}, len {}",
                    data.len()
                )));
            }

            let is_live = cs.chan_type == 1;
            if is_live {
                live_chan_idx += 1;
            }

            if !live_only || is_live {
                seq += 1;
                let mut samples = Vec::with_capacity(ns);
                let raw = &data[data_start..data_end];
                let scale = cs.mp_descaling;
                for chunk in raw.chunks_exact(4) {
                    samples.push(f32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) * scale);
                }

                let len_ms = if ns > 1 {
                    (ns - 1) as f32 * (summary.dt_us as f32 / 1000.0)
                } else {
                    0.0
                };
                let mut h = TraceHeaders::default();
                h.ffid = ffid;
                h.source = ffid;
                h.sin = ffid;
                h.chan = if is_live {
                    if chan_bcd > 0 { chan_bcd } else { live_chan_idx }
                } else {
                    chan_bcd
                };
                h.nchans = summary.live_channels as i32;
                h.seqno = seq;
                h.trc_type = if is_live {
                    TraceType::Live as i32
                } else {
                    TraceType::Auxiliary as i32
                };
                if let Some(sx) = summary.sou_x {
                    h.sou_x = sx;
                }
                if let Some(sy) = summary.sou_y {
                    h.sou_y = sy;
                }
                if let Some(wd) = summary.water_depth {
                    h.sou_h2od = wd;
                }
                h.tlive_s = 0.0;
                h.tfull_s = 0.0;
                h.tfull_e = len_ms;
                h.tlive_e = len_ms;
                h.custom.insert("SEGD_CS_NUM".into(), cs.cs_num as f64);

                traces.push(Trace {
                    dt_us: summary.dt_us,
                    headers: h,
                    samples,
                });
            }

            offset = data_end;
        }
    }

    if let Some(last) = traces.last_mut() {
        last.headers.end_ens = 1;
    }

    Ok((summary, traces))
}

/// Read a directory of `.segd` files in parallel (sorted by filename), optionally limiting shot count.
pub fn read_segd_dir(
    dir: impl AsRef<Path>,
    max_shots: Option<usize>,
    live_only: bool,
) -> Result<(Vec<SegdSummary>, Vec<Trace>)> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir.as_ref())?
        .filter_map(|e| e.ok().map(|x| x.path()))
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("segd"))
                .unwrap_or(false)
        })
        .collect();
    paths.sort();
    if let Some(max_n) = max_shots {
        paths.truncate(max_n);
    }

    let results: Result<Vec<(SegdSummary, Vec<Trace>)>> = paths
        .par_iter()
        .map(|p| read_segd_file(p, live_only))
        .collect();
    let results = results?;

    let mut summaries = Vec::with_capacity(results.len());
    let mut all_traces = Vec::new();
    let mut traceno = 0i32;

    for (shot_idx, (sum, mut trs)) in results.into_iter().enumerate() {
        summaries.push(sum);
        for tr in &mut trs {
            traceno += 1;
            tr.headers.traceno = traceno;
            tr.headers.sin = (shot_idx + 1) as i32;
            tr.headers.eoj = 0;
        }
        all_traces.extend(trs);
    }
    if let Some(last) = all_traces.last_mut() {
        last.headers.eoj = 1;
    }

    Ok((summaries, all_traces))
}
