use rustseis_core::{Result, SeismicError, Trace, TraceHeaders, TraceType};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Endianness {
    Big,
    Little,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SegySummary {
    pub endianness: Endianness,
    pub format_code: u16,
    pub dt_us: u32,
    pub n_samples: usize,
    pub n_traces: usize,
    pub text_header_preview: String,
}

/// Convert 32-bit IBM System/360 floating point to IEEE 754 `f32`.
#[inline]
pub fn ibm_to_ieee_f32(ibm: u32) -> f32 {
    if ibm == 0 {
        return 0.0;
    }
    let sign = (ibm >> 31) != 0;
    let exp = ((ibm >> 24) & 0x7F) as i32;
    let frac = (ibm & 0x00FF_FFFF) as f64 / 16_777_216.0; // 2^24
    let val = frac * 16.0_f64.powi(exp - 64);
    if sign { -(val as f32) } else { val as f32 }
}

/// Convert IEEE 754 `f32` to 32-bit IBM System/360 floating point.
#[inline]
pub fn ieee_to_ibm_u32(val: f32) -> u32 {
    if val == 0.0 || !val.is_finite() {
        return 0;
    }
    let sign = if val < 0.0 { 1u32 << 31 } else { 0u32 };
    let mut abs = val.abs() as f64;
    let mut exp = 64i32;
    while abs >= 1.0 {
        abs /= 16.0;
        exp += 1;
    }
    while abs < 1.0 / 16.0 && exp > 0 {
        abs *= 16.0;
        exp -= 1;
    }
    let frac = ((abs * 16_777_216.0).round() as u32).min(0x00FF_FFFF);
    sign | (((exp.clamp(0, 127) as u32) & 0x7F) << 24) | frac
}

#[inline]
fn read_i16(b: &[u8], off: usize, end: Endianness) -> i16 {
    let arr = [b[off], b[off + 1]];
    match end {
        Endianness::Big => i16::from_be_bytes(arr),
        Endianness::Little => i16::from_le_bytes(arr),
    }
}

#[inline]
fn read_u16(b: &[u8], off: usize, end: Endianness) -> u16 {
    let arr = [b[off], b[off + 1]];
    match end {
        Endianness::Big => u16::from_be_bytes(arr),
        Endianness::Little => u16::from_le_bytes(arr),
    }
}

#[inline]
fn read_i32(b: &[u8], off: usize, end: Endianness) -> i32 {
    let arr = [b[off], b[off + 1], b[off + 2], b[off + 3]];
    match end {
        Endianness::Big => i32::from_be_bytes(arr),
        Endianness::Little => i32::from_le_bytes(arr),
    }
}

#[inline]
fn read_u32(b: &[u8], off: usize, end: Endianness) -> u32 {
    let arr = [b[off], b[off + 1], b[off + 2], b[off + 3]];
    match end {
        Endianness::Big => u32::from_be_bytes(arr),
        Endianness::Little => u32::from_le_bytes(arr),
    }
}

#[inline]
fn apply_segy_scalar(val: i32, scalar: i16) -> f64 {
    if scalar > 0 {
        val as f64 * scalar as f64
    } else if scalar < 0 {
        val as f64 / (-(scalar as i32) as f64)
    } else {
        val as f64
    }
}

fn detect_endianness_and_binary_header(bh: &[u8]) -> Result<(Endianness, u16, u32, usize)> {
    let fmt_be = u16::from_be_bytes([bh[24], bh[25]]);
    let dt_be = u16::from_be_bytes([bh[16], bh[17]]) as u32;
    let ns_be = u16::from_be_bytes([bh[20], bh[21]]) as usize;

    let fmt_le = u16::from_le_bytes([bh[24], bh[25]]);
    let dt_le = u16::from_le_bytes([bh[16], bh[17]]) as u32;
    let ns_le = u16::from_le_bytes([bh[20], bh[21]]) as usize;

    let valid_fmt = |f: u16| matches!(f, 1 | 2 | 3 | 5 | 8);
    if valid_fmt(fmt_be) && (dt_be > 0 || ns_be > 0) {
        Ok((Endianness::Big, fmt_be, dt_be, ns_be))
    } else if valid_fmt(fmt_le) && (dt_le > 0 || ns_le > 0) {
        Ok((Endianness::Little, fmt_le, dt_le, ns_le))
    } else {
        Err(SeismicError::InvalidFormat(format!(
            "Could not detect valid SEG-Y format (BE fmt={fmt_be} dt={dt_be} ns={ns_be}; LE fmt={fmt_le} dt={dt_le} ns={ns_le})"
        )))
    }
}

/// CP037/CP500 EBCDIC -> ASCII table for industry SEG-Y 3200-byte textual headers.
const EBCDIC_TO_ASCII: [u8; 256] = [
    32,32,32,32,32,32,32,32,32,32,32,32,32,32,32,32,
    32,32,32,32,32,10,32,32,32,32,32,32,32,32,32,32,
    32,32,32,32,32,10,32,32,32,32,32,32,32,32,32,32,
    32,32,32,32,32,32,32,32,32,32,32,32,32,32,32,32,
    b' ',32,32,32,32,32,32,32,32,32,b'[',b'.',b'<',b'(',b'+',b'!',
    b'&',32,32,32,32,32,32,32,32,32,b']',b'$',b'*',b')',b';',b'^',
    b'-',b'/',32,32,32,32,32,32,32,32,b'|',b',',b'%',b'_',b'>',b'?',
    32,32,32,32,32,32,32,32,32,b'`',b':',b'#',b'@',b'\'',b'=',b'"',
    32,b'a',b'b',b'c',b'd',b'e',b'f',b'g',b'h',b'i',32,32,32,32,32,32,
    32,b'j',b'k',b'l',b'm',b'n',b'o',b'p',b'q',b'r',32,32,32,32,32,32,
    32,b'~',b's',b't',b'u',b'v',b'w',b'x',b'y',b'z',32,32,32,32,32,32,
    32,32,32,32,32,32,32,32,32,32,32,32,32,32,32,32,
    b'{',b'A',b'B',b'C',b'D',b'E',b'F',b'G',b'H',b'I',32,32,32,32,32,32,
    b'}',b'J',b'K',b'L',b'M',b'N',b'O',b'P',b'Q',b'R',32,32,32,32,32,32,
    b'\\',32,b'S',b'T',b'U',b'V',b'W',b'X',b'Y',b'Z',32,32,32,32,32,32,
    b'0',b'1',b'2',b'3',b'4',b'5',b'6',b'7',b'8',b'9',32,32,32,32,32,32,
];

fn decode_text_preview(txt: &[u8]) -> String {
    let ebcdic_spaces = txt.iter().take(320).filter(|&&b| b == 0x40).count();
    let ascii_spaces = txt.iter().take(320).filter(|&&b| b == 0x20).count();
    let decoded: String = if ebcdic_spaces > ascii_spaces && ebcdic_spaces > 10 {
        txt.iter()
            .take(400)
            .map(|&b| EBCDIC_TO_ASCII[b as usize] as char)
            .collect()
    } else {
        txt.iter()
            .take(400)
            .map(|&b| if (32..=126).contains(&b) { b as char } else { ' ' })
            .collect()
    };
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Inspect a SEG-Y file header without reading all traces.
pub fn inspect_segy_file(path: impl AsRef<Path>) -> Result<SegySummary> {
    let data = fs::read(path.as_ref())?;
    if data.len() < 3600 {
        return Err(SeismicError::InvalidFormat("SEG-Y file smaller than 3600 bytes".into()));
    }
    let (end, fmt, mut dt_us, mut ns) = detect_endianness_and_binary_header(&data[3200..3600])?;
    if data.len() >= 3840 {
        let th = &data[3600..3840];
        if ns == 0 {
            ns = read_u16(th, 114, end) as usize;
        }
        if dt_us == 0 {
            dt_us = read_u16(th, 116, end) as u32;
        }
    }
    let bps = match fmt {
        1 | 2 | 5 => 4,
        3 => 2,
        8 => 1,
        _ => 4,
    };
    let tr_bytes = 240 + ns * bps;
    let n_traces = if tr_bytes > 240 {
        (data.len() - 3600) / tr_bytes
    } else {
        0
    };
    Ok(SegySummary {
        endianness: end,
        format_code: fmt,
        dt_us,
        n_samples: ns,
        n_traces,
        text_header_preview: decode_text_preview(&data[0..3200]),
    })
}

/// Read all traces (or up to `max_traces`) from a SEG-Y Rev 0/1/2 file (BE or LE, IBM or IEEE).
pub fn read_segy_file(
    path: impl AsRef<Path>,
    max_traces: Option<usize>,
) -> Result<(SegySummary, Vec<Trace>)> {
    let data = fs::read(path.as_ref())?;
    if data.len() < 3600 {
        return Err(SeismicError::InvalidFormat("SEG-Y file smaller than 3600 bytes".into()));
    }
    let (end, fmt, bin_dt_us, bin_ns) = detect_endianness_and_binary_header(&data[3200..3600])?;
    let bps = match fmt {
        1 | 2 | 5 => 4,
        3 => 2,
        8 => 1,
        other => {
            return Err(SeismicError::InvalidFormat(format!(
                "Unsupported SEG-Y sample format code {other}"
            )));
        }
    };

    let mut traces = Vec::new();
    let mut off = 3600usize;
    let mut count = 0usize;

    while off + 240 <= data.len() {
        if let Some(limit) = max_traces {
            if count >= limit {
                break;
            }
        }
        let th = &data[off..off + 240];
        let t_ns = read_u16(th, 114, end) as usize;
        let ns = if t_ns > 0 { t_ns } else { bin_ns };
        let t_dt = read_u16(th, 116, end) as u32;
        let dt_us = if t_dt > 0 { t_dt } else { bin_dt_us };
        if ns == 0 {
            break;
        }
        let data_start = off + 240;
        let data_end = data_start + ns * bps;
        if data_end > data.len() {
            break;
        }

        let mut samples = Vec::with_capacity(ns);
        let raw = &data[data_start..data_end];
        match fmt {
            1 => {
                for i in 0..ns {
                    samples.push(ibm_to_ieee_f32(read_u32(raw, i * 4, end)));
                }
            }
            2 => {
                for i in 0..ns {
                    samples.push(read_i32(raw, i * 4, end) as f32);
                }
            }
            3 => {
                for i in 0..ns {
                    samples.push(read_i16(raw, i * 2, end) as f32);
                }
            }
            5 => {
                for i in 0..ns {
                    samples.push(f32::from_bits(read_u32(raw, i * 4, end)));
                }
            }
            8 => {
                for &b in raw {
                    samples.push((b as i8) as f32);
                }
            }
            _ => unreachable!(),
        }

        let scalel = read_i16(th, 68, end);
        let scalco = read_i16(th, 70, end);
        let trid = read_i16(th, 28, end);
        let len_ms = if ns > 1 {
            (ns - 1) as f32 * (dt_us as f32 / 1000.0)
        } else {
            0.0
        };

        let mut h = TraceHeaders::default();
        h.traceno = read_i32(th, 0, end);
        h.ffid = read_i32(th, 8, end);
        h.chan = read_i32(th, 12, end);
        h.source = read_i32(th, 16, end);
        h.cdp = read_i32(th, 20, end);
        h.seqno = read_i32(th, 24, end);
        h.trc_type = match trid {
            2 => TraceType::Dead as i32,
            3..=9 => TraceType::Auxiliary as i32,
            _ => TraceType::Live as i32,
        };
        h.tr_fold = read_i16(th, 30, end).max(1) as i32;
        h.offset = read_i32(th, 36, end) as f32;
        h.aoffset = h.offset.abs();
        h.rec_elev = apply_segy_scalar(read_i32(th, 40, end), scalel) as f32;
        h.sou_elev = apply_segy_scalar(read_i32(th, 44, end), scalel) as f32;
        h.depth = apply_segy_scalar(read_i32(th, 48, end), scalel) as f32;
        h.sou_h2od = apply_segy_scalar(read_i32(th, 60, end), scalel) as f32;
        h.rec_h2od = apply_segy_scalar(read_i32(th, 64, end), scalel) as f32;
        h.sou_x = apply_segy_scalar(read_i32(th, 72, end), scalco);
        h.sou_y = apply_segy_scalar(read_i32(th, 76, end), scalco);
        h.rec_x = apply_segy_scalar(read_i32(th, 80, end), scalco);
        h.rec_y = apply_segy_scalar(read_i32(th, 84, end), scalco);
        h.uphole = read_i16(th, 94, end) as f32;
        h.sou_stat = read_i16(th, 98, end) as f32;
        h.rec_stat = read_i16(th, 100, end) as f32;
        h.tot_stat = read_i16(th, 102, end) as f32;
        h.cdp_x = apply_segy_scalar(read_i32(th, 180, end), scalco);
        h.cdp_y = apply_segy_scalar(read_i32(th, 184, end), scalco);
        h.iline_no = read_i32(th, 188, end);
        h.xline_no = read_i32(th, 192, end);
        h.tlive_s = 0.0;
        h.tfull_s = 0.0;
        h.tfull_e = len_ms;
        h.tlive_e = len_ms;

        if h.traceno == 0 {
            h.traceno = (count + 1) as i32;
        }

        traces.push(Trace {
            dt_us,
            headers: h,
            samples,
        });
        off = data_end;
        count += 1;
    }

    if let Some(last) = traces.last_mut() {
        last.headers.eoj = 1;
    }

    let summary = SegySummary {
        endianness: end,
        format_code: fmt,
        dt_us: traces.first().map(|t| t.dt_us).unwrap_or(bin_dt_us),
        n_samples: traces.first().map(|t| t.samples.len()).unwrap_or(bin_ns),
        n_traces: traces.len(),
        text_header_preview: decode_text_preview(&data[0..3200]),
    };

    Ok((summary, traces))
}

/// Write traces to a standard Big-Endian SEG-Y Rev 1 IEEE float32 (format code 5) file.
pub fn write_segy_file(path: impl AsRef<Path>, traces: &[Trace], text_comment: &str) -> Result<()> {
    if traces.is_empty() {
        return Err(SeismicError::InvalidFormat("Cannot write empty trace list to SEG-Y".into()));
    }
    let dt_us = traces[0].dt_us;
    let ns = traces[0].samples.len();

    let file = File::create(path.as_ref())?;
    let mut w = BufWriter::new(file);

    // 3200-byte ASCII textual header
    let mut txt = vec![b' '; 3200];
    let header_msg = format!(
        "C 1 CLIENT: RUSTSEIS  EXPORT: SEG-Y REV 1 IEEE FLOAT32                          C 2 {text_comment}"
    );
    let bytes = header_msg.as_bytes();
    let n_copy = bytes.len().min(3200);
    txt[..n_copy].copy_from_slice(&bytes[..n_copy]);
    w.write_all(&txt)?;

    // 400-byte binary header (big-endian)
    let mut bh = [0u8; 400];
    bh[12..14].copy_from_slice(&1u16.to_be_bytes()); // traces/ensemble placeholder
    bh[16..18].copy_from_slice(&(dt_us as u16).to_be_bytes());
    bh[20..22].copy_from_slice(&(ns as u16).to_be_bytes());
    bh[24..26].copy_from_slice(&5u16.to_be_bytes()); // IEEE float32
    bh[54..56].copy_from_slice(&1u16.to_be_bytes()); // meters
    bh[300..302].copy_from_slice(&0x0100u16.to_be_bytes()); // Rev 1.0
    bh[302..304].copy_from_slice(&1u16.to_be_bytes()); // fixed length traces
    w.write_all(&bh)?;

    // Write each trace (240-byte header + IEEE f32 big-endian samples)
    for (idx, tr) in traces.iter().enumerate() {
        let h = &tr.headers;
        let mut th = [0u8; 240];
        let tracl = if h.traceno > 0 { h.traceno } else { (idx + 1) as i32 };
        th[0..4].copy_from_slice(&tracl.to_be_bytes());
        th[4..8].copy_from_slice(&((idx + 1) as i32).to_be_bytes());
        th[8..12].copy_from_slice(&h.ffid.to_be_bytes());
        th[12..16].copy_from_slice(&h.chan.to_be_bytes());
        th[16..20].copy_from_slice(&h.source.to_be_bytes());
        th[20..24].copy_from_slice(&h.cdp.to_be_bytes());
        th[24..28].copy_from_slice(&h.seqno.to_be_bytes());
        let trid: i16 = if h.trc_type == TraceType::Dead as i32 {
            2
        } else if h.trc_type == TraceType::Auxiliary as i32 {
            3
        } else {
            1
        };
        th[28..30].copy_from_slice(&trid.to_be_bytes());
        th[30..32].copy_from_slice(&(h.tr_fold.clamp(1, i16::MAX as i32) as i16).to_be_bytes());
        th[36..40].copy_from_slice(&(h.offset.round() as i32).to_be_bytes());
        // Use scalar -100 (cm precision) for coords & elevations
        let scal: i16 = -100;
        th[68..70].copy_from_slice(&scal.to_be_bytes());
        th[70..72].copy_from_slice(&scal.to_be_bytes());
        th[40..44].copy_from_slice(&((h.rec_elev as f64 * 100.0).round() as i32).to_be_bytes());
        th[44..48].copy_from_slice(&((h.sou_elev as f64 * 100.0).round() as i32).to_be_bytes());
        th[48..52].copy_from_slice(&((h.depth as f64 * 100.0).round() as i32).to_be_bytes());
        th[60..64].copy_from_slice(&((h.sou_h2od as f64 * 100.0).round() as i32).to_be_bytes());
        th[64..68].copy_from_slice(&((h.rec_h2od as f64 * 100.0).round() as i32).to_be_bytes());
        th[72..76].copy_from_slice(&((h.sou_x * 100.0).round() as i32).to_be_bytes());
        th[76..80].copy_from_slice(&((h.sou_y * 100.0).round() as i32).to_be_bytes());
        th[80..84].copy_from_slice(&((h.rec_x * 100.0).round() as i32).to_be_bytes());
        th[84..88].copy_from_slice(&((h.rec_y * 100.0).round() as i32).to_be_bytes());
        th[88..90].copy_from_slice(&1i16.to_be_bytes()); // length units (meters)
        th[98..100].copy_from_slice(&(h.sou_stat.round() as i16).to_be_bytes());
        th[100..102].copy_from_slice(&(h.rec_stat.round() as i16).to_be_bytes());
        th[102..104].copy_from_slice(&(h.tot_stat.round() as i16).to_be_bytes());
        th[114..116].copy_from_slice(&(tr.samples.len() as u16).to_be_bytes());
        th[116..118].copy_from_slice(&(tr.dt_us as u16).to_be_bytes());
        th[180..184].copy_from_slice(&((h.cdp_x * 100.0).round() as i32).to_be_bytes());
        th[184..188].copy_from_slice(&((h.cdp_y * 100.0).round() as i32).to_be_bytes());
        th[188..192].copy_from_slice(&h.iline_no.to_be_bytes());
        th[192..196].copy_from_slice(&h.xline_no.to_be_bytes());
        w.write_all(&th)?;

        for &s in &tr.samples {
            w.write_all(&s.to_be_bytes())?;
        }
    }
    w.flush()?;
    Ok(())
}
