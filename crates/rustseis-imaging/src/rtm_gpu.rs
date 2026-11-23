//! Native `wgpu` WGSL compute-shader backend for 2D Acoustic Reverse Time Migration (RTM).
//!
//! Keeps wavefields, velocity/sponge grids, source snapshots, and imaging accumulators resident
//! in GPU VRAM across the entire shot time loop — zero per-step PCIe transfers!

use crate::modeling::ricker_wavelet;
use crate::rtm::{PaddedGrid, Rtm2dParams, ShotTask};
use bytemuck::{Pod, Zeroable};
use rustseis_core::{JobControl, Result, SeismicError};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GridUniform {
    pnx: u32,
    pnz: u32,
    nx: u32,
    nz: u32,
    pad: u32,
    inv_dx2: f32,
    inv_dz2: f32,
    _pad0: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct StepUniform {
    a: u32,
    b: u32,
    c: u32,
    d: u32,
}

const WGSL_RTM: &str = r#"
struct GridParams {
    pnx: u32,
    pnz: u32,
    nx: u32,
    nz: u32,
    pad: u32,
    inv_dx2: f32,
    inv_dz2: f32,
    _pad0: u32,
};

struct StepParams {
    a: u32, // snap_idx (save/correlate) or n_rec (inject_rec) or src_idx (inject_src)
    b: u32, // step_idx (inject_rec) or f32 bits of amp (inject_src)
    c: u32, // ns (inject_rec)
    d: u32, // sub_steps (inject_rec)
};

@group(0) @binding(0) var<uniform> grid: GridParams;
@group(0) @binding(1) var<storage, read> v2dt2: array<f32>;
@group(0) @binding(2) var<storage, read> sponge: array<f32>;

// Ping-pong wavefields
@group(1) @binding(0) var<storage, read> p_prev: array<f32>;
@group(1) @binding(1) var<storage, read> p_curr: array<f32>;
@group(1) @binding(2) var<storage, read_write> p_next: array<f32>;

// Auxiliary storage for snapshots, receivers, image, and step params
@group(2) @binding(0) var<storage, read_write> snaps: array<f32>;
@group(2) @binding(1) var<storage, read_write> shot_img: array<f32>;
@group(2) @binding(2) var<storage, read_write> shot_illum: array<f32>;
@group(2) @binding(3) var<storage, read> rec_indices: array<u32>;
@group(2) @binding(4) var<storage, read> rec_data: array<f32>;
@group(2) @binding(5) var<uniform> sp: StepParams;

const C0: f32 = -2.84722222;
const C1: f32 =  1.60000000;
const C2: f32 = -0.20000000;
const C3: f32 =  0.02539683;
const C4: f32 = -0.00178571;

@compute @workgroup_size(16, 16, 1)
fn fd_step(@builtin(global_invocation_id) gid: vec3<u32>) {
    let px = gid.x;
    let pz = gid.y;
    if (px >= grid.pnx || pz >= grid.pnz) {
        return;
    }
    let idx = pz * grid.pnx + px;
    if (px < 4u || px + 4u >= grid.pnx || pz < 4u || pz + 4u >= grid.pnz) {
        p_next[idx] = 0.0;
        return;
    }
    let pnx = grid.pnx;
    let c = p_curr[idx];
    let d2x = C0 * c
        + C1 * (p_curr[idx - 1u] + p_curr[idx + 1u])
        + C2 * (p_curr[idx - 2u] + p_curr[idx + 2u])
        + C3 * (p_curr[idx - 3u] + p_curr[idx + 3u])
        + C4 * (p_curr[idx - 4u] + p_curr[idx + 4u]);
    let d2z = C0 * c
        + C1 * (p_curr[idx - pnx] + p_curr[idx + pnx])
        + C2 * (p_curr[idx - 2u * pnx] + p_curr[idx + 2u * pnx])
        + C3 * (p_curr[idx - 3u * pnx] + p_curr[idx + 3u * pnx])
        + C4 * (p_curr[idx - 4u * pnx] + p_curr[idx + 4u * pnx]);
    let lap = d2x * grid.inv_dx2 + d2z * grid.inv_dz2;
    p_next[idx] = (2.0 * c - p_prev[idx] + v2dt2[idx] * lap) * sponge[idx];
}

@compute @workgroup_size(1, 1, 1)
fn inject_source() {
    let s_idx = sp.a;
    let amp = bitcast<f32>(sp.b);
    p_next[s_idx] = p_next[s_idx] + amp * v2dt2[s_idx];
}

@compute @workgroup_size(64, 1, 1)
fn inject_receivers(@builtin(global_invocation_id) gid: vec3<u32>) {
    let ri = gid.x;
    if (ri >= sp.a) {
        return;
    }
    let step = sp.b;
    let ns = sp.c;
    let sub = sp.d;
    let f_it = f32(step) / f32(sub);
    let it0 = min(u32(floor(f_it)), ns - 1u);
    let it1 = min(it0 + 1u, ns - 1u);
    let frac = f_it - f32(it0);
    let a0 = rec_data[ri * ns + it0];
    let a1 = rec_data[ri * ns + it1];
    let val = a0 + frac * (a1 - a0);
    let r_idx = rec_indices[ri];
    p_next[r_idx] = p_next[r_idx] + val * v2dt2[r_idx];
}

@compute @workgroup_size(16, 16, 1)
fn save_snapshot(@builtin(global_invocation_id) gid: vec3<u32>) {
    let ix = gid.x;
    let iz = gid.y;
    if (ix >= grid.nx || iz >= grid.nz) {
        return;
    }
    let src_idx = (iz + grid.pad) * grid.pnx + (ix + grid.pad);
    let dst_idx = sp.a * (grid.nx * grid.nz) + iz * grid.nx + ix;
    snaps[dst_idx] = p_next[src_idx];
}

@compute @workgroup_size(16, 16, 1)
fn correlate_snapshot(@builtin(global_invocation_id) gid: vec3<u32>) {
    let ix = gid.x;
    let iz = gid.y;
    if (ix >= grid.nx || iz >= grid.nz) {
        return;
    }
    let r_idx = (iz + grid.pad) * grid.pnx + (ix + grid.pad);
    let i_idx = iz * grid.nx + ix;
    let s = snaps[sp.a * (grid.nx * grid.nz) + i_idx];
    let r = p_next[r_idx];
    shot_img[i_idx] = shot_img[i_idx] + s * r;
    shot_illum[i_idx] = shot_illum[i_idx] + s * s;
}
"#;

pub(crate) fn rtm_gpu(
    g: &PaddedGrid,
    shots: &[ShotTask],
    ns: usize,
    params: &Rtm2dParams,
    job: Option<&JobControl>,
) -> Result<(Vec<f32>, String)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .ok_or_else(|| SeismicError::NotFound("No GPU adapter found for wgpu RTM".into()))?;

    let info = adapter.get_info();
    let adapter_name = info.name.clone();
    let adapter_limits = adapter.limits();

    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("rustseis-rtm-gpu"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits {
                max_storage_buffer_binding_size: adapter_limits.max_storage_buffer_binding_size,
                max_buffer_size: adapter_limits.max_buffer_size,
                ..wgpu::Limits::default()
            },
            memory_hints: wgpu::MemoryHints::Performance,
        },
        None,
    ))
    .map_err(|e| SeismicError::Numerical(format!("wgpu device error: {e}")))?;

    let n_fd = ns * g.sub_steps;
    let mut snap_sample_stride = params.snapshot_sample_stride.max(1);
    let img_bytes = (g.nx * g.nz * 4) as u64;
    let max_snap_bytes = (device.limits().max_storage_buffer_binding_size as u64)
        .min(device.limits().max_buffer_size)
        .min(512 * 1024 * 1024);
    while ((ns + snap_sample_stride - 1) / snap_sample_stride) as u64 * img_bytes > max_snap_bytes {
        snap_sample_stride *= 2;
    }
    let snap_every_fd = g.sub_steps * snap_sample_stride;
    let n_snaps = ((n_fd + snap_every_fd - 1) / snap_every_fd).max(1);

    let grid_u = GridUniform {
        pnx: g.pnx as u32,
        pnz: g.pnz as u32,
        nx: g.nx as u32,
        nz: g.nz as u32,
        pad: g.pad as u32,
        inv_dx2: g.inv_dx2,
        inv_dz2: g.inv_dz2,
        _pad0: 0,
    };

    let grid_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("grid_u"),
        contents: bytemuck::bytes_of(&grid_u),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let v2dt2_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("v2dt2"),
        contents: bytemuck::cast_slice(&g.v2dt2),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let sponge_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("sponge"),
        contents: bytemuck::cast_slice(&g.sponge),
        usage: wgpu::BufferUsages::STORAGE,
    });

    let field_bytes = (g.pnx * g.pnz * 4) as u64;
    let mk_field = |lbl: &str| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(lbl),
            size: field_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    let p_bufs = [mk_field("p0"), mk_field("p1"), mk_field("p2")];

    let snaps_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("snaps"),
        size: (n_snaps as u64 * img_bytes).max(4),
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let shot_img_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("shot_img"),
        size: img_bytes.max(4),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let shot_illum_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("shot_illum"),
        size: img_bytes.max(4),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let read_img_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("read_img"),
        size: img_bytes.max(4),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let read_illum_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("read_illum"),
        size: img_bytes.max(4),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let max_recs = shots.iter().map(|s| s.rec_cells.len()).max().unwrap_or(1).max(1);
    let rec_idx_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rec_indices"),
        size: (max_recs * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let rec_data_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rec_data"),
        size: (max_recs * ns * 4).max(4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let align = device.limits().min_uniform_buffer_offset_alignment.max(256) as usize;
    let max_step_slots = (n_fd + n_snaps + 4).max(16);
    let step_u_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("step_u_dyn"),
        size: (max_step_slots * align) as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut step_staging = vec![0u8; max_step_slots * align];

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("rtm_wgsl"),
        source: wgpu::ShaderSource::Wgsl(WGSL_RTM.into()),
    });

    let bgl0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bgl0"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
        ],
    });
    let bgl1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bgl1"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
        ],
    });
    let bgl2 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bgl2"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<StepUniform>() as u64),
                },
                count: None,
            },
        ],
    });

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("rtm_pl"),
        bind_group_layouts: &[&bgl0, &bgl1, &bgl2],
        push_constant_ranges: &[],
    });

    let mk_pipe = |entry: &str| {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: Some(&layout),
            module: &shader,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        })
    };
    let pipe_fd = mk_pipe("fd_step");
    let pipe_src = mk_pipe("inject_source");
    let pipe_rec = mk_pipe("inject_receivers");
    let pipe_save = mk_pipe("save_snapshot");
    let pipe_corr = mk_pipe("correlate_snapshot");

    let bg0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bg0"),
        layout: &bgl0,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: grid_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: v2dt2_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: sponge_buf.as_entire_binding() },
        ],
    });
    // 3 cyclic ping-pong bind groups: (0,1->2), (1,2->0), (2,0->1)
    let bg1 = [0usize, 1, 2].map(|rot| {
        let i_prev = rot % 3;
        let i_curr = (rot + 1) % 3;
        let i_next = (rot + 2) % 3;
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg1"),
            layout: &bgl1,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: p_bufs[i_prev].as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: p_bufs[i_curr].as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: p_bufs[i_next].as_entire_binding() },
            ],
        })
    });
    let bg2 = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bg2"),
        layout: &bgl2,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: snaps_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: shot_img_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: shot_illum_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: rec_idx_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 4, resource: rec_data_buf.as_entire_binding() },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &step_u_buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<StepUniform>() as u64),
                }),
            },
        ],
    });

    let wg_pad_x = (g.pnx as u32 + 15) / 16;
    let wg_pad_z = (g.pnz as u32 + 15) / 16;
    let wg_img_x = (g.nx as u32 + 15) / 16;
    let wg_img_z = (g.nz as u32 + 15) / 16;
    let t_delay_s = 1.2 / params.dominant_freq_hz.max(5.0) as f64;
    let zeros_field = vec![0.0f32; g.pnx * g.pnz];
    let zeros_img = vec![0.0f32; g.nx * g.nz];
    let mut stack_img = vec![0.0f64; g.nx * g.nz];

    for (si, shot) in shots.iter().enumerate() {
        if let Some(j) = job {
            j.check_cancelled()?;
            j.set_progress(
                si as f32 / shots.len().max(1) as f32,
                format!("RTM shot {}/{} (GPU: {adapter_name})", si + 1, shots.len()),
            );
        }

        // Upload shot receiver layout & traces once per shot
        let r_indices: Vec<u32> = shot
            .rec_cells
            .iter()
            .map(|&(rpx, rpz)| (rpz * g.pnx + rpx) as u32)
            .collect();
        queue.write_buffer(&rec_idx_buf, 0, bytemuck::cast_slice(&r_indices));
        queue.write_buffer(&rec_data_buf, 0, bytemuck::cast_slice(&shot.rec_data));
        for b in &p_bufs {
            queue.write_buffer(b, 0, bytemuck::cast_slice(&zeros_field));
        }

        // 1. Forward propagation (ONE batched GPU submission for the entire shot!)
        let s_idx = (shot.src_cell.1 * g.pnx + shot.src_cell.0) as u32;
        let mut slot = 0usize;
        let mut fwd_ops: Vec<(usize, u32, Option<u32>)> = Vec::with_capacity(n_fd);
        for step in 0..n_fd {
            let t_s = step as f64 * g.dt_fd_s;
            let w = ricker_wavelet(t_s - t_delay_s, params.dominant_freq_hz as f64) as f32;
            let su = StepUniform { a: s_idx, b: w.to_bits(), c: 0, d: 0 };
            let off_src = (slot * align) as u32;
            step_staging[slot * align..slot * align + 16].copy_from_slice(bytemuck::bytes_of(&su));
            slot += 1;

            let off_snap = if step % snap_every_fd == 0 && (step / snap_every_fd) < n_snaps {
                let k = (step / snap_every_fd) as u32;
                let su_s = StepUniform { a: k, b: 0, c: 0, d: 0 };
                let off = (slot * align) as u32;
                step_staging[slot * align..slot * align + 16].copy_from_slice(bytemuck::bytes_of(&su_s));
                slot += 1;
                Some(off)
            } else {
                None
            };
            fwd_ops.push((step % 3, off_src, off_snap));
        }
        queue.write_buffer(&step_u_buf, 0, &step_staging[..slot * align]);

        let mut enc_fwd = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc_fwd.begin_compute_pass(&Default::default());
            pass.set_bind_group(0, &bg0, &[]);
            for &(rot, off_src, off_snap) in &fwd_ops {
                pass.set_bind_group(1, &bg1[rot], &[]);
                pass.set_bind_group(2, &bg2, &[off_src]);
                pass.set_pipeline(&pipe_fd);
                pass.dispatch_workgroups(wg_pad_x, wg_pad_z, 1);
                pass.set_pipeline(&pipe_src);
                pass.dispatch_workgroups(1, 1, 1);
                if let Some(off_s) = off_snap {
                    pass.set_bind_group(2, &bg2, &[off_s]);
                    pass.set_pipeline(&pipe_save);
                    pass.dispatch_workgroups(wg_img_x, wg_img_z, 1);
                }
            }
        }
        queue.submit(Some(enc_fwd.finish()));

        // 2. Backward propagation + cross-correlation (ONE batched GPU submission!)
        for b in &p_bufs {
            queue.write_buffer(b, 0, bytemuck::cast_slice(&zeros_field));
        }
        queue.write_buffer(&shot_img_buf, 0, bytemuck::cast_slice(&zeros_img));
        queue.write_buffer(&shot_illum_buf, 0, bytemuck::cast_slice(&zeros_img));
        let n_rec = shot.rec_cells.len() as u32;
        let wg_rec = (n_rec + 63) / 64;

        slot = 0;
        let mut bwd_ops: Vec<(usize, u32, Option<u32>)> = Vec::with_capacity(n_fd);
        for rev in 0..n_fd {
            let step = n_fd - 1 - rev;
            let su = StepUniform {
                a: n_rec,
                b: step as u32,
                c: ns as u32,
                d: g.sub_steps as u32,
            };
            let off_rec = (slot * align) as u32;
            step_staging[slot * align..slot * align + 16].copy_from_slice(bytemuck::bytes_of(&su));
            slot += 1;

            let off_corr = if step % snap_every_fd == 0 && (step / snap_every_fd) < n_snaps {
                let k = (step / snap_every_fd) as u32;
                let su_c = StepUniform { a: k, b: 0, c: 0, d: 0 };
                let off = (slot * align) as u32;
                step_staging[slot * align..slot * align + 16].copy_from_slice(bytemuck::bytes_of(&su_c));
                slot += 1;
                Some(off)
            } else {
                None
            };
            bwd_ops.push((rev % 3, off_rec, off_corr));
        }
        queue.write_buffer(&step_u_buf, 0, &step_staging[..slot * align]);

        let mut enc_bwd = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc_bwd.begin_compute_pass(&Default::default());
            pass.set_bind_group(0, &bg0, &[]);
            for &(rot, off_rec, off_corr) in &bwd_ops {
                pass.set_bind_group(1, &bg1[rot], &[]);
                pass.set_bind_group(2, &bg2, &[off_rec]);
                pass.set_pipeline(&pipe_fd);
                pass.dispatch_workgroups(wg_pad_x, wg_pad_z, 1);
                if n_rec > 0 {
                    pass.set_pipeline(&pipe_rec);
                    pass.dispatch_workgroups(wg_rec, 1, 1);
                }
                if let Some(off_c) = off_corr {
                    pass.set_bind_group(2, &bg2, &[off_c]);
                    pass.set_pipeline(&pipe_corr);
                    pass.dispatch_workgroups(wg_img_x, wg_img_z, 1);
                }
            }
        }
        enc_bwd.copy_buffer_to_buffer(&shot_img_buf, 0, &read_img_buf, 0, img_bytes);
        enc_bwd.copy_buffer_to_buffer(&shot_illum_buf, 0, &read_illum_buf, 0, img_bytes);
        queue.submit(Some(enc_bwd.finish()));

        let s_img = read_img_buf.slice(..);
        let s_il = read_illum_buf.slice(..);
        let (tx1, rx1) = std::sync::mpsc::channel();
        let (tx2, rx2) = std::sync::mpsc::channel();
        s_img.map_async(wgpu::MapMode::Read, move |r| { let _ = tx1.send(r); });
        s_il.map_async(wgpu::MapMode::Read, move |r| { let _ = tx2.send(r); });
        device.poll(wgpu::Maintain::Wait);
        rx1.recv().ok().and_then(|r| r.ok()).ok_or_else(|| SeismicError::Numerical("GPU map img failed".into()))?;
        rx2.recv().ok().and_then(|r| r.ok()).ok_or_else(|| SeismicError::Numerical("GPU map illum failed".into()))?;

        {
            let v_img = s_img.get_mapped_range();
            let v_il = s_il.get_mapped_range();
            let f_img: &[f32] = bytemuck::cast_slice(&v_img);
            let f_il: &[f32] = bytemuck::cast_slice(&v_il);
            let max_il = f_il.iter().copied().fold(0.0f32, f32::max).max(1e-30) as f64;
            let eps = params.illumination_eps.max(1e-6) as f64 * max_il;
            for i in 0..(g.nx * g.nz) {
                stack_img[i] += f_img[i] as f64 / (f_il[i] as f64 + eps);
            }
        }
        read_img_buf.unmap();
        read_illum_buf.unmap();
    }

    Ok((stack_img.into_iter().map(|v| v as f32).collect(), adapter_name))
}
