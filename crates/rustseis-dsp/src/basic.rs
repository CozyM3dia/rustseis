use rayon::prelude::*;
use rustseis_core::{MuteMode, MuteTable, Result, SeismicError, Trace};
use serde::{Deserialize, Serialize};
use std::f32::consts::PI;

/// Remove DC bias over the live sample window (`debias`).
pub fn debias_trace(trace: &mut Trace) {
    if !trace.is_live() || trace.samples.is_empty() {
        return;
    }
    let (s, e) = trace.live_sample_range();
    let slice = &trace.samples[s..e];
    if slice.is_empty() {
        return;
    }
    let mean = (slice.iter().map(|&v| v as f64).sum::<f64>() / slice.len() as f64) as f32;
    for v in &mut trace.samples[s..e] {
        *v -= mean;
    }
}

pub fn debias_traces(traces: &mut [Trace]) {
    traces.par_iter_mut().for_each(debias_trace);
}

/// Truncate or zero-pad traces to a target record length in milliseconds (`trace_len`).
pub fn set_trace_length(trace: &mut Trace, target_ms: f32) {
    let dt_ms = trace.dt_ms().max(1e-6);
    let target_n = ((target_ms / dt_ms).round() as usize) + 1;
    trace.samples.resize(target_n, 0.0);
    let max_ms = trace.duration_ms();
    trace.headers.tfull_e = trace.headers.tfull_e.min(max_ms);
    trace.headers.tlive_e = trace.headers.tlive_e.min(max_ms);
    if trace.headers.tlive_e == 0.0 {
        trace.headers.tlive_e = max_ms;
        trace.headers.tfull_e = max_ms;
    }
}

/// Band-limited Kaiser/Hann-windowed sinc resampling (`resamp`).
pub fn resample_trace(trace: &mut Trace, new_dt_us: u32) -> Result<()> {
    if new_dt_us == 0 || trace.dt_us == 0 {
        return Err(SeismicError::InvalidParameter {
            param: "new_dt_us".into(),
            message: "Sample interval must be > 0".into(),
        });
    }
    if new_dt_us == trace.dt_us || trace.samples.is_empty() {
        return Ok(());
    }
    let old_dt = trace.dt_us as f64;
    let new_dt = new_dt_us as f64;
    let dur = (trace.samples.len() - 1) as f64 * old_dt;
    let new_len = (dur / new_dt).floor() as usize + 1;
    let ratio = old_dt / new_dt; // < 1 when downsampling
    let cutoff = ratio.min(1.0);
    let half_width: isize = 12;
    let n_old = trace.samples.len() as isize;

    let mut out = vec![0.0f32; new_len];
    for i in 0..new_len {
        let t_old = (i as f64 * new_dt) / old_dt;
        let center = t_old.round() as isize;
        let mut acc = 0.0f64;
        let mut wsum = 0.0f64;
        for k in (center - half_width)..=(center + half_width) {
            if k < 0 || k >= n_old {
                continue;
            }
            let x = t_old - k as f64;
            let sinc = if x.abs() < 1e-9 {
                1.0
            } else {
                let px = std::f64::consts::PI * x * cutoff;
                px.sin() / px
            };
            let win = 0.5
                * (1.0
                    + (std::f64::consts::PI * x / (half_width as f64 + 1.0)).cos());
            let w = sinc * win * cutoff;
            acc += trace.samples[k as usize] as f64 * w;
            wsum += w;
        }
        out[i] = if wsum.abs() > 1e-12 {
            (acc / wsum) as f32
        } else {
            0.0
        };
    }
    trace.samples = out;
    trace.dt_us = new_dt_us;
    Ok(())
}

/// Trace kill/reverse rules (`trc_edit`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEditParams {
    #[serde(default)]
    pub kill_channels: Vec<i32>,
    #[serde(default)]
    pub kill_ffids: Vec<i32>,
    #[serde(default)]
    pub kill_tracenos: Vec<i32>,
    #[serde(default)]
    pub reverse_channels: Vec<i32>,
    #[serde(default)]
    pub reverse_ffids: Vec<i32>,
}

pub fn apply_trace_edit(traces: &mut [Trace], params: &TraceEditParams) {
    traces.par_iter_mut().for_each(|tr| {
        if params.kill_channels.contains(&tr.headers.chan)
            || params.kill_ffids.contains(&tr.headers.ffid)
            || params.kill_tracenos.contains(&tr.headers.traceno)
        {
            tr.kill();
            return;
        }
        if params.reverse_channels.contains(&tr.headers.chan)
            || params.reverse_ffids.contains(&tr.headers.ffid)
        {
            tr.reverse_polarity();
        }
    });
}

/// Apply Top, Bottom, or Surgical mute with smooth cosine taper (`mute`),
/// updating ProMAX `TLIVE_S`, `TFULL_S`, `TFULL_E`, `TLIVE_E`, `LEN_SURG` headers.
pub fn apply_mute_trace(trace: &mut Trace, table: &MuteTable) {
    if !trace.is_live() || trace.samples.is_empty() {
        return;
    }
    let dt_ms = trace.dt_ms().max(1e-6);
    let key = trace.headers.get_f64(&table.key_header).unwrap_or(0.0) as f32;
    let taper_ms = table.taper_ms.max(0.0);
    let n = trace.samples.len();

    match table.mode {
        MuteMode::Top => {
            if let Some(t_mute) = table.time_at(key) {
                let t_full = t_mute + taper_ms;
                for i in 0..n {
                    let t = i as f32 * dt_ms;
                    if t <= t_mute {
                        trace.samples[i] = 0.0;
                    } else if t < t_full && taper_ms > 0.0 {
                        let frac = (t - t_mute) / taper_ms;
                        let w = 0.5 * (1.0 - (PI * frac).cos());
                        trace.samples[i] *= w;
                    }
                }
                trace.headers.tlive_s = trace.headers.tlive_s.max(t_mute);
                trace.headers.tfull_s = trace.headers.tfull_s.max(t_full);
            }
        }
        MuteMode::Bottom => {
            if let Some(t_mute) = table.time_at(key) {
                let t_full = (t_mute - taper_ms).max(0.0);
                for i in 0..n {
                    let t = i as f32 * dt_ms;
                    if t >= t_mute {
                        trace.samples[i] = 0.0;
                    } else if t > t_full && taper_ms > 0.0 {
                        let frac = (t_mute - t) / taper_ms;
                        let w = 0.5 * (1.0 - (PI * frac).cos());
                        trace.samples[i] *= w;
                    }
                }
                if trace.headers.tlive_e == 0.0 || t_mute < trace.headers.tlive_e {
                    trace.headers.tlive_e = t_mute;
                    trace.headers.tfull_e = t_full;
                }
            }
        }
        MuteMode::Surgical => {
            if let Some((t1, t2)) = table.surgical_window_at(key) {
                let (t_lo, t_hi) = if t1 <= t2 { (t1, t2) } else { (t2, t1) };
                for i in 0..n {
                    let t = i as f32 * dt_ms;
                    if t >= t_lo && t <= t_hi {
                        trace.samples[i] = 0.0;
                    } else if taper_ms > 0.0 && t > t_lo - taper_ms && t < t_lo {
                        let frac = (t_lo - t) / taper_ms;
                        trace.samples[i] *= 0.5 * (1.0 - (PI * frac).cos());
                    } else if taper_ms > 0.0 && t > t_hi && t < t_hi + taper_ms {
                        let frac = (t - t_hi) / taper_ms;
                        trace.samples[i] *= 0.5 * (1.0 - (PI * frac).cos());
                    }
                }
                trace.headers.len_surg = taper_ms;
            }
        }
    }
}

pub fn apply_mute(traces: &mut [Trace], table: &MuteTable) {
    traces.par_iter_mut().for_each(|tr| apply_mute_trace(tr, table));
}

/// Apply fractional-sample static time shift in milliseconds (positive = shift down/later).
pub fn shift_trace_ms(trace: &mut Trace, shift_ms: f32) {
    if shift_ms.abs() < 1e-6 || trace.samples.is_empty() {
        return;
    }
    let dt_ms = trace.dt_ms().max(1e-6);
    let shift_samp = shift_ms as f64 / dt_ms as f64;
    let n = trace.samples.len();
    let mut out = vec![0.0f32; n];
    let half = 8isize;
    for i in 0..n {
        let src = i as f64 - shift_samp;
        if src < 0.0 || src > (n - 1) as f64 {
            continue;
        }
        let c = src.round() as isize;
        let mut acc = 0.0f64;
        let mut wsum = 0.0f64;
        for k in (c - half)..=(c + half) {
            if k < 0 || k >= n as isize {
                continue;
            }
            let x = src - k as f64;
            let sinc = if x.abs() < 1e-9 {
                1.0
            } else {
                let px = std::f64::consts::PI * x;
                px.sin() / px
            };
            let win = 0.5 * (1.0 + (std::f64::consts::PI * x / (half as f64 + 1.0)).cos());
            let w = sinc * win;
            acc += trace.samples[k as usize] as f64 * w;
            wsum += w;
        }
        out[i] = if wsum.abs() > 1e-12 { (acc / wsum) as f32 } else { 0.0 };
    }
    trace.samples = out;
}

/// Elevation & Datum Statics (`elev_stat` / `datum_stat_calc` / `datum_stat_apply`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElevationStaticsParams {
    pub final_datum_m: f32,
    pub replacement_velocity_mps: f32,
    #[serde(default = "default_true")]
    pub apply_shift: bool,
}

fn default_true() -> bool {
    true
}

pub fn apply_elevation_statics(traces: &mut [Trace], params: &ElevationStaticsParams) {
    let v_rep = params.replacement_velocity_mps.max(100.0);
    traces.par_iter_mut().for_each(|tr| {
        let s_stat_ms = ((params.final_datum_m - tr.headers.sou_elev) / v_rep) * 1000.0 - tr.headers.uphole;
        let r_stat_ms = ((params.final_datum_m - tr.headers.rec_elev) / v_rep) * 1000.0;
        let tot = s_stat_ms + r_stat_ms;
        tr.headers.sou_stat = s_stat_ms;
        tr.headers.rec_stat = r_stat_ms;
        tr.headers.tot_stat += tot;
        if params.apply_shift {
            shift_trace_ms(tr, tot);
        } else {
            tr.headers.na_stat += tot;
        }
    });
}

/// Simple safe header expression evaluator (`thdrmath`), supporting
/// `TARGET = A + B * C`, unary `-`, parentheses, `abs(...)`, constants, and all 91 header names.
pub fn apply_header_math(traces: &mut [Trace], target_header: &str, expr: &str) -> Result<()> {
    let tokens = tokenize_expr(expr)?;
    for tr in traces.iter_mut() {
        let mut pos = 0;
        let val = parse_add(&tokens, &mut pos, &tr.headers)?;
        tr.headers.set_f64(target_header, val);
    }
    Ok(())
}

#[derive(Debug, Clone)]
enum Tok {
    Num(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
}

fn tokenize_expr(s: &str) -> Result<Vec<Tok>> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '+' => { out.push(Tok::Plus); i += 1; }
            '-' => { out.push(Tok::Minus); i += 1; }
            '*' => { out.push(Tok::Star); i += 1; }
            '/' => { out.push(Tok::Slash); i += 1; }
            '(' => { out.push(Tok::LParen); i += 1; }
            ')' => { out.push(Tok::RParen); i += 1; }
            '0'..='9' | '.' => {
                let start = i;
                while i < bytes.len() && ((bytes[i] as char).is_ascii_digit() || bytes[i] == b'.') {
                    i += 1;
                }
                let v: f64 = s[start..i].parse().map_err(|_| SeismicError::InvalidParameter {
                    param: "expr".into(),
                    message: format!("Invalid number '{}'", &s[start..i]),
                })?;
                out.push(Tok::Num(v));
            }
            'a'..='z' | 'A'..='Z' | '_' => {
                let start = i;
                while i < bytes.len()
                    && ((bytes[i] as char).is_ascii_alphanumeric() || bytes[i] == b'_')
                {
                    i += 1;
                }
                out.push(Tok::Ident(s[start..i].to_string()));
            }
            _ => {
                return Err(SeismicError::InvalidParameter {
                    param: "expr".into(),
                    message: format!("Unexpected char '{c}'"),
                });
            }
        }
    }
    Ok(out)
}

fn parse_add(toks: &[Tok], pos: &mut usize, h: &rustseis_core::TraceHeaders) -> Result<f64> {
    let mut left = parse_mul(toks, pos, h)?;
    while *pos < toks.len() {
        match toks[*pos] {
            Tok::Plus => { *pos += 1; left += parse_mul(toks, pos, h)?; }
            Tok::Minus => { *pos += 1; left -= parse_mul(toks, pos, h)?; }
            _ => break,
        }
    }
    Ok(left)
}

fn parse_mul(toks: &[Tok], pos: &mut usize, h: &rustseis_core::TraceHeaders) -> Result<f64> {
    let mut left = parse_unary(toks, pos, h)?;
    while *pos < toks.len() {
        match toks[*pos] {
            Tok::Star => { *pos += 1; left *= parse_unary(toks, pos, h)?; }
            Tok::Slash => {
                *pos += 1;
                let rhs = parse_unary(toks, pos, h)?;
                left = if rhs.abs() < 1e-12 { 0.0 } else { left / rhs };
            }
            _ => break,
        }
    }
    Ok(left)
}

fn parse_unary(toks: &[Tok], pos: &mut usize, h: &rustseis_core::TraceHeaders) -> Result<f64> {
    if *pos < toks.len() {
        match toks[*pos] {
            Tok::Minus => { *pos += 1; return Ok(-parse_unary(toks, pos, h)?); }
            Tok::Plus => { *pos += 1; return parse_unary(toks, pos, h); }
            _ => {}
        }
    }
    parse_primary(toks, pos, h)
}

fn parse_primary(toks: &[Tok], pos: &mut usize, h: &rustseis_core::TraceHeaders) -> Result<f64> {
    if *pos >= toks.len() {
        return Err(SeismicError::InvalidParameter {
            param: "expr".into(),
            message: "Unexpected end of expression".into(),
        });
    }
    match &toks[*pos] {
        Tok::Num(v) => { let x = *v; *pos += 1; Ok(x) }
        Tok::LParen => {
            *pos += 1;
            let v = parse_add(toks, pos, h)?;
            if matches!(toks.get(*pos), Some(Tok::RParen)) {
                *pos += 1;
            }
            Ok(v)
        }
        Tok::Ident(name) => {
            let n = name.clone();
            *pos += 1;
            if matches!(toks.get(*pos), Some(Tok::LParen)) {
                *pos += 1;
                let arg = parse_add(toks, pos, h)?;
                if matches!(toks.get(*pos), Some(Tok::RParen)) {
                    *pos += 1;
                }
                match n.to_ascii_lowercase().as_str() {
                    "abs" => Ok(arg.abs()),
                    "sqrt" => Ok(arg.max(0.0).sqrt()),
                    "floor" => Ok(arg.floor()),
                    "round" => Ok(arg.round()),
                    _ => Err(SeismicError::NotFound(format!("Unknown function '{n}'"))),
                }
            } else {
                Ok(h.get_f64(&n).unwrap_or(0.0))
            }
        }
        _ => Err(SeismicError::InvalidParameter {
            param: "expr".into(),
            message: "Invalid primary token".into(),
        }),
    }
}
