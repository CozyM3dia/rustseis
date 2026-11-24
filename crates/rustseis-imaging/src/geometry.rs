use rustseis_core::{CrookedLineGeometry2D, Ensemble, LandGeometry2D, MarineGeometry2D, Trace};
use std::collections::HashMap;

/// Assign 2D Marine Streamer Geometry (`geomspread_marine` / `geom_load`).
/// Computes receiver coordinates trailing the shot along the sail line, signed offsets,
/// CDP midpoint coordinates, CDP bin numbers, and CDP fold.
pub fn assign_marine_geometry_2d(traces: &mut [Trace], geom: &MarineGeometry2D) {
    if traces.is_empty() {
        return;
    }

    // Collect ordered unique shot positions (by FFID) to determine sail-line origin & unit direction
    let mut shots: Vec<(i32, f64, f64)> = Vec::new();
    let mut seen = HashMap::<i32, usize>::new();
    for tr in traces.iter() {
        let ffid = tr.headers.ffid;
        if !seen.contains_key(&ffid) {
            seen.insert(ffid, shots.len());
            shots.push((ffid, tr.headers.sou_x, tr.headers.sou_y));
        }
    }

    let has_coords = shots.iter().any(|s| s.1.abs() > 1e-3 || s.2.abs() > 1e-3);
    let (origin_x, origin_y, ux, uy) = if has_coords && shots.len() >= 2 {
        let first = shots[0];
        let last = shots[shots.len() - 1];
        let dx = last.1 - first.1;
        let dy = last.2 - first.2;
        let dist = dx.hypot(dy).max(1e-6);
        (first.1, first.2, dx / dist, dy / dist)
    } else {
        let az = geom.sail_line_azimuth_deg.to_radians();
        // Azimuth clockwise from North (+Y): ux = sin(az), uy = cos(az). 90° => (1, 0).
        (geom.first_shot_x, geom.first_shot_y, az.sin(), az.cos())
    };
    // Crossline unit vector perpendicular to sail direction (right-hand side)
    let (px, py) = (uy, -ux);

    // Determine max channel count for reversed streamer ordering
    let max_chan = traces.iter().map(|t| t.headers.chan).max().unwrap_or(1).max(1);

    // First pass: compute sou/rec/cdp coords, offsets, and along-line midpoint distance
    let mut min_mid_s = f64::INFINITY;
    let mut mid_s_vals = vec![0.0f64; traces.len()];

    let vw = geom.water_velocity_mps.max(500.0);
    let src_stat_ms = if geom.apply_sea_level_static {
        (geom.source_depth_m / vw) * 1000.0
    } else {
        0.0
    };
    let rec_stat_ms = if geom.apply_sea_level_static {
        (geom.streamer_depth_m / vw) * 1000.0
    } else {
        0.0
    };

    for (idx, tr) in traces.iter_mut().enumerate() {
        let shot_idx = *seen.get(&tr.headers.ffid).unwrap_or(&0);
        let (sx, sy) = if has_coords && (tr.headers.sou_x.abs() > 1e-3 || tr.headers.sou_y.abs() > 1e-3) {
            (tr.headers.sou_x, tr.headers.sou_y)
        } else {
            let s = shot_idx as f64 * geom.nominal_shot_interval_m;
            (origin_x + s * ux, origin_y + s * uy)
        };

        let ch_idx = if geom.chan1_is_near {
            (tr.headers.chan - 1).max(0) as f64
        } else {
            (max_chan - tr.headers.chan).max(0) as f64
        };
        let inline_offset = geom.near_offset_m + ch_idx * geom.receiver_interval_m;
        let offset = if geom.perp_offset_m.abs() > 1e-6 {
            inline_offset.hypot(geom.perp_offset_m)
        } else {
            inline_offset
        };

        // Streamer trails behind the vessel along (-ux, -uy) with optional perpendicular offset
        let rx = sx - inline_offset * ux + geom.perp_offset_m * px;
        let ry = sy - inline_offset * uy + geom.perp_offset_m * py;
        let mx = 0.5 * (sx + rx);
        let my = 0.5 * (sy + ry);

        let mid_s = (mx - origin_x) * ux + (my - origin_y) * uy;
        mid_s_vals[idx] = mid_s;
        if mid_s < min_mid_s {
            min_mid_s = mid_s;
        }

        tr.headers.sou_x = sx;
        tr.headers.sou_y = sy;
        tr.headers.rec_x = rx;
        tr.headers.rec_y = ry;
        tr.headers.cdp_x = mx;
        tr.headers.cdp_y = my;
        tr.headers.offset = offset as f32;
        tr.headers.aoffset = offset.abs() as f32;
        tr.headers.depth = geom.source_depth_m;
        tr.headers.rec_dep = geom.streamer_depth_m;
        tr.headers.sou_sloc = if geom.first_shot_station > 0 {
            geom.first_shot_station + shot_idx as i32
        } else {
            tr.headers.ffid
        };
        tr.headers.rec_sloc = ((mid_s / geom.receiver_interval_m.max(1e-3)).round()) as i32;
        if geom.apply_sea_level_static {
            let tot = src_stat_ms + rec_stat_ms;
            tr.headers.sou_stat = src_stat_ms;
            tr.headers.rec_stat = rec_stat_ms;
            tr.headers.na_stat += tot;
            tr.headers.tot_stat += tot;
        }
    }

    // Second pass: assign CDP bin numbers starting at 1 and populate CDP fold
    let bin_size = geom.cdp_bin_size_m.max(0.1);
    let mut cdp_fold = HashMap::<i32, i32>::new();
    for (idx, tr) in traces.iter_mut().enumerate() {
        let rcdp = ((mid_s_vals[idx] - min_mid_s) / bin_size) + 1.0;
        let cdp = rcdp.round() as i32;
        tr.headers.rcdp = rcdp as f32;
        tr.headers.cdp = cdp;
        tr.headers.cdp_sloc = cdp;
        if tr.is_live() {
            *cdp_fold.entry(cdp).or_insert(0) += 1;
        }
    }
    for tr in traces.iter_mut() {
        tr.headers.cdp_nfld = *cdp_fold.get(&tr.headers.cdp).unwrap_or(&0);
    }
}

/// Assign 2D Straight-Line Land Geometry (`geomspread_land`).
pub fn assign_land_geometry_2d(traces: &mut [Trace], geom: &LandGeometry2D) {
    let station_map: HashMap<i32, (f64, f64, f32)> = geom
        .stations
        .iter()
        .map(|&(id, x, y, z)| (id, (x, y, z)))
        .collect();
    if station_map.is_empty() || traces.is_empty() {
        return;
    }
    let first = geom.stations[0];
    let last = geom.stations[geom.stations.len() - 1];
    let dx = last.1 - first.1;
    let dy = last.2 - first.2;
    let dist = dx.hypot(dy).max(1e-6);
    let (ux, uy) = (dx / dist, dy / dist);

    let bin_size = geom.cdp_bin_size_m.max(0.1);
    let mut cdp_fold = HashMap::<i32, i32>::new();
    for tr in traces.iter_mut() {
        if let (Some(&(sx, sy, sz)), Some(&(rx, ry, rz))) = (
            station_map.get(&tr.headers.sou_sloc),
            station_map.get(&tr.headers.rec_sloc),
        ) {
            let mx = 0.5 * (sx + rx);
            let my = 0.5 * (sy + ry);
            let along_s = (rx - sx) * ux + (ry - sy) * uy;
            let dist_sr = (rx - sx).hypot(ry - sy);
            let offset = if along_s < 0.0 { -dist_sr } else { dist_sr };
            let mid_s = (mx - first.1) * ux + (my - first.2) * uy;
            let cdp = (mid_s / bin_size).round() as i32 + 1;
            tr.headers.sou_x = sx;
            tr.headers.sou_y = sy;
            tr.headers.sou_elev = sz;
            tr.headers.rec_x = rx;
            tr.headers.rec_y = ry;
            tr.headers.rec_elev = rz;
            tr.headers.cdp_x = mx;
            tr.headers.cdp_y = my;
            tr.headers.cdp_elev = 0.5 * (sz + rz);
            tr.headers.offset = offset as f32;
            tr.headers.aoffset = dist_sr as f32;
            tr.headers.cdp = cdp;
            tr.headers.cdp_sloc = cdp;
            if tr.is_live() {
                *cdp_fold.entry(cdp).or_insert(0) += 1;
            }
        }
    }
    for tr in traces.iter_mut() {
        tr.headers.cdp_nfld = *cdp_fold.get(&tr.headers.cdp).unwrap_or(&0);
    }
}

/// Assign 2D Crooked-Line (Slalom-Line) Geometry (`geomspread_crooked`).
/// Projects shot-receiver midpoints onto a curved piecewise-linear slalom processing line,
/// assigns arc-length CDP bins, records signed crossline offset in `custom["XLINE_DIST"]`,
/// computes true Euclidean offset & azimuth, and flags traces outside `crossline_half_width_m`.
pub fn assign_crooked_line_geometry_2d(traces: &mut [Trace], geom: &CrookedLineGeometry2D) {
    if traces.is_empty() || geom.stations.is_empty() {
        return;
    }
    let station_map: HashMap<i32, (f64, f64, f32)> = geom
        .stations
        .iter()
        .map(|&(id, x, y, z)| (id, (x, y, z)))
        .collect();

    // 1. Populate source/receiver/midpoint coords from station table (if present) or existing headers
    let mut midpoints = Vec::with_capacity(traces.len());
    for tr in traces.iter_mut() {
        if let Some(&(sx, sy, sz)) = station_map.get(&tr.headers.sou_sloc) {
            tr.headers.sou_x = sx;
            tr.headers.sou_y = sy;
            tr.headers.sou_elev = sz;
        }
        if let Some(&(rx, ry, rz)) = station_map.get(&tr.headers.rec_sloc) {
            tr.headers.rec_x = rx;
            tr.headers.rec_y = ry;
            tr.headers.rec_elev = rz;
        }
        let mx = 0.5 * (tr.headers.sou_x + tr.headers.rec_x);
        let my = 0.5 * (tr.headers.sou_y + tr.headers.rec_y);
        tr.headers.cdp_x = mx;
        tr.headers.cdp_y = my;
        tr.headers.cdp_elev = 0.5 * (tr.headers.sou_elev + tr.headers.rec_elev);
        let dx = tr.headers.rec_x - tr.headers.sou_x;
        let dy = tr.headers.rec_y - tr.headers.sou_y;
        let dist = dx.hypot(dy);
        tr.headers.aoffset = dist as f32;
        tr.headers.sr_azim = dy.atan2(dx).to_degrees() as f32;
        midpoints.push((mx, my));
    }

    // 2. Build or adopt the slalom polyline vertices
    let mut poly: Vec<(f64, f64)> = if geom.slalom_vertices.len() >= 2 {
        geom.slalom_vertices.clone()
    } else {
        // Auto-build smooth slalom line through station path
        let mut pts: Vec<(f64, f64)> = geom.stations.iter().map(|&(_, x, y, _)| (x, y)).collect();
        for _ in 0..geom.auto_smooth_passes {
            if pts.len() < 3 {
                break;
            }
            let mut next = Vec::with_capacity(pts.len());
            next.push(pts[0]);
            for w in pts.windows(3) {
                next.push((
                    0.25 * w[0].0 + 0.5 * w[1].0 + 0.25 * w[2].0,
                    0.25 * w[0].1 + 0.5 * w[1].1 + 0.25 * w[2].1,
                ));
            }
            next.push(pts[pts.len() - 1]);
            pts = next;
        }
        pts
    };
    if poly.len() < 2 {
        return;
    }

    // Remove degenerate zero-length segments
    poly.dedup_by(|a, b| (a.0 - b.0).hypot(a.1 - b.1) < 1e-6);
    if poly.len() < 2 {
        return;
    }

    // Cumulative arc-length along polyline
    let mut cum_s = vec![0.0f64; poly.len()];
    for i in 1..poly.len() {
        cum_s[i] = cum_s[i - 1] + (poly[i].0 - poly[i - 1].0).hypot(poly[i].1 - poly[i - 1].1);
    }

    // 3. Project every midpoint onto nearest segment of the slalom polyline
    let bin_size = geom.cdp_bin_size_m.max(0.1);
    let max_xline = geom.crossline_half_width_m.max(1.0);
    let mut cdp_fold = HashMap::<i32, i32>::new();

    for (idx, tr) in traces.iter_mut().enumerate() {
        let (mx, my) = midpoints[idx];
        let mut best_dist_sq = f64::INFINITY;
        let mut best_arc_s = 0.0f64;
        let mut best_signed_xline = 0.0f64;
        let mut best_tangent = (1.0f64, 0.0f64);

        for i in 0..(poly.len() - 1) {
            let (ax, ay) = poly[i];
            let (bx, by) = poly[i + 1];
            let vx = bx - ax;
            let vy = by - ay;
            let seg_len_sq = vx * vx + vy * vy;
            if seg_len_sq < 1e-12 {
                continue;
            }
            let t = (((mx - ax) * vx + (my - ay) * vy) / seg_len_sq).clamp(0.0, 1.0);
            let px = ax + t * vx;
            let py = ay + t * vy;
            let rx = mx - px;
            let ry = my - py;
            let d2 = rx * rx + ry * ry;
            if d2 < best_dist_sq {
                best_dist_sq = d2;
                let seg_len = seg_len_sq.sqrt();
                let ux = vx / seg_len;
                let uy = vy / seg_len;
                best_arc_s = cum_s[i] + t * seg_len;
                // Signed crossline distance (2D cross product)
                best_signed_xline = ux * ry - uy * rx;
                best_tangent = (ux, uy);
            }
        }

        let along_sr = (tr.headers.rec_x - tr.headers.sou_x) * best_tangent.0
            + (tr.headers.rec_y - tr.headers.sou_y) * best_tangent.1;
        tr.headers.offset = if along_sr < 0.0 {
            -tr.headers.aoffset
        } else {
            tr.headers.aoffset
        };

        let rcdp = (best_arc_s / bin_size) + 1.0;
        let cdp = rcdp.round() as i32;
        tr.headers.rcdp = rcdp as f32;
        tr.headers.cdp = cdp;
        tr.headers.cdp_sloc = cdp;
        tr.headers
            .custom
            .insert("XLINE_DIST".into(), best_signed_xline);
        tr.headers.custom.insert("SLALOM_S".into(), best_arc_s);

        if best_signed_xline.abs() > max_xline {
            tr.headers.trc_type = 0; // Outside crooked-line bin corridor -> flag dead
        } else if tr.is_live() {
            *cdp_fold.entry(cdp).or_insert(0) += 1;
        }
    }

    for tr in traces.iter_mut() {
        tr.headers.cdp_nfld = *cdp_fold.get(&tr.headers.cdp).unwrap_or(&0);
    }
}

/// Inline Sort (`inline_sort`): sort traces by primary and optional secondary ProMAX header key,
/// updating `END_ENS`, `SEQNO`, and `EOJ`.
pub fn inline_sort(
    mut traces: Vec<Trace>,
    primary_key: &str,
    secondary_key: Option<&str>,
    primary_desc: bool,
    secondary_desc: bool,
) -> Vec<Trace> {
    traces.sort_by(|a, b| {
        let va = a.headers.get_f64(primary_key).unwrap_or(0.0);
        let vb = b.headers.get_f64(primary_key).unwrap_or(0.0);
        let ord1 = if primary_desc {
            vb.total_cmp(&va)
        } else {
            va.total_cmp(&vb)
        };
        if ord1 != std::cmp::Ordering::Equal {
            return ord1;
        }
        if let Some(sk) = secondary_key {
            let sa = a.headers.get_f64(sk).unwrap_or(0.0);
            let sb = b.headers.get_f64(sk).unwrap_or(0.0);
            if secondary_desc {
                sb.total_cmp(&sa)
            } else {
                sa.total_cmp(&sb)
            }
        } else {
            std::cmp::Ordering::Equal
        }
    });

    let ensembles = Ensemble::group_by_header(traces, primary_key);
    Ensemble::flatten(ensembles)
}
