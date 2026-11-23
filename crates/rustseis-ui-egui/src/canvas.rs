//! Data-space canvas shared by every raster view (gathers, sections, semblance, CVS, F-K, Radon,
//! VSP). A view owns a [`CanvasState`]; the canvas maps data units to pixels, draws cached
//! textures by their data extent, paints axes and the crosshair, and handles pan / zoom.

use crate::data::RasterExtent;
use crate::theme::{Palette, font_mono, with_alpha};
use egui::{
    Color32, CornerRadius, Mesh, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind,
    TextureHandle, Ui, Vec2, pos2, vec2,
};
use rustseis_core::Trace;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View2D {
    pub x0: f64,
    pub x1: f64,
    pub y0: f64,
    pub y1: f64,
}

impl View2D {
    pub fn new(x0: f64, x1: f64, y0: f64, y1: f64) -> Self {
        Self { x0, x1, y0, y1 }
    }
    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }
    fn is_valid(&self) -> bool {
        self.x1 > self.x0 && self.y1 > self.y0 && self.x0.is_finite() && self.y1.is_finite()
    }
}

/// Screen <-> data mapping for one frame.
#[derive(Clone, Copy, Debug)]
pub struct PlotFrame {
    pub rect: Rect,
    pub view: View2D,
    /// `true`: y grows downward (time, depth). `false`: y grows upward (frequency, velocity plots).
    pub y_down: bool,
}

impl PlotFrame {
    pub fn x_to_screen(&self, x: f64) -> f32 {
        self.rect.left() + ((x - self.view.x0) / self.view.width()) as f32 * self.rect.width()
    }
    pub fn y_to_screen(&self, y: f64) -> f32 {
        let t = ((y - self.view.y0) / self.view.height()) as f32;
        if self.y_down {
            self.rect.top() + t * self.rect.height()
        } else {
            self.rect.bottom() - t * self.rect.height()
        }
    }
    pub fn to_screen(&self, x: f64, y: f64) -> Pos2 {
        pos2(self.x_to_screen(x), self.y_to_screen(y))
    }
    pub fn x_from_screen(&self, sx: f32) -> f64 {
        self.view.x0 + ((sx - self.rect.left()) / self.rect.width()) as f64 * self.view.width()
    }
    pub fn y_from_screen(&self, sy: f32) -> f64 {
        let t = if self.y_down {
            (sy - self.rect.top()) / self.rect.height()
        } else {
            (self.rect.bottom() - sy) / self.rect.height()
        };
        self.view.y0 + t as f64 * self.view.height()
    }
    pub fn from_screen(&self, p: Pos2) -> (f64, f64) {
        (self.x_from_screen(p.x), self.y_from_screen(p.y))
    }
    /// Pixels per data unit along x.
    pub fn px_per_x(&self) -> f32 {
        self.rect.width() / self.view.width() as f32
    }
}

/// A GPU texture plus the data extent its outer texel edges cover.
pub struct Raster {
    pub tex: TextureHandle,
    pub extent: RasterExtent,
}

impl Raster {
    pub fn new(ctx: &egui::Context, name: &str, img: egui::ColorImage, extent: RasterExtent) -> Self {
        let tex = ctx.load_texture(name, img, egui::TextureOptions::LINEAR);
        Self { tex, extent }
    }
    pub fn replace(&mut self, img: egui::ColorImage, extent: RasterExtent) {
        self.tex.set(img, egui::TextureOptions::LINEAR);
        self.extent = extent;
    }
}

/// Draw `raster` clipped to the visible view; texels outside the view are never sampled.
pub fn draw_raster(painter: &Painter, frame: &PlotFrame, raster: &Raster, tint: Color32) {
    let e = raster.extent;
    let (ex0, ex1) = (e.x0.min(e.x1), e.x0.max(e.x1));
    let (ey0, ey1) = (e.y0.min(e.y1), e.y0.max(e.y1));
    let vx0 = frame.view.x0.max(ex0);
    let vx1 = frame.view.x1.min(ex1);
    let vy0 = frame.view.y0.max(ey0);
    let vy1 = frame.view.y1.min(ey1);
    if vx1 <= vx0 || vy1 <= vy0 {
        return;
    }
    let u = |x: f64| ((x - e.x0) / (e.x1 - e.x0)) as f32;
    let v = |y: f64| ((y - e.y0) / (e.y1 - e.y0)) as f32;
    let p0 = frame.to_screen(vx0, vy0);
    let p1 = frame.to_screen(vx1, vy1);
    let rect = Rect::from_two_pos(p0, p1);
    // Extent y0/y1 are the data values at the texture's top/bottom edges (either order), so
    // v(y) is the texture coordinate of any data y. The screen rect's top edge shows the
    // data y at the top of the view, which depends on the axis orientation.
    let (y_top, y_bot) = if frame.y_down { (vy0, vy1) } else { (vy1, vy0) };
    let uv = Rect::from_min_max(pos2(u(vx0), v(y_top)), pos2(u(vx1), v(y_bot)));
    painter.image(raster.tex.id(), rect, uv, tint);
}

/// "Nice" 1-2-5 tick positions covering `[v0, v1]`.
pub fn nice_ticks(v0: f64, v1: f64, target: usize) -> Vec<f64> {
    let (lo, hi) = (v0.min(v1), v0.max(v1));
    let span = hi - lo;
    if !(span.is_finite()) || span <= 0.0 {
        return Vec::new();
    }
    let raw = span / target.max(1) as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let step = mag * if norm < 1.5 { 1.0 } else if norm < 3.5 { 2.0 } else if norm < 7.5 { 5.0 } else { 10.0 };
    let mut t = (lo / step).ceil() * step;
    let mut out = Vec::new();
    while t <= hi + step * 1e-9 && out.len() < 200 {
        out.push(if t.abs() < step * 1e-9 { 0.0 } else { t });
        t += step;
    }
    out
}

pub type AxisLabel<'a> = Box<dyn Fn(f64) -> Option<String> + 'a>;

pub struct CanvasOpts<'a> {
    pub y_down: bool,
    pub x_label: AxisLabel<'a>,
    pub y_label: AxisLabel<'a>,
    pub x_title: String,
    pub y_title: String,
    pub x_axis_top: bool,
    pub lock_x: bool,
    pub lock_y: bool,
    /// Primary-button drag pans (navigation mode). Middle drag always pans.
    pub primary_pans: bool,
    pub show_grid: bool,
    pub show_hover_crosshair: bool,
    pub left_gutter: f32,
    pub x_ticks: usize,
    pub y_ticks: usize,
    pub background: Color32,
}

impl<'a> CanvasOpts<'a> {
    pub fn seismic(x_title: impl Into<String>) -> Self {
        Self {
            y_down: true,
            x_label: Box::new(|x| Some(format!("{x:.0}"))),
            y_label: Box::new(|y| Some(format!("{y:.0}"))),
            x_title: x_title.into(),
            y_title: "ms".into(),
            x_axis_top: true,
            lock_x: false,
            lock_y: false,
            primary_pans: true,
            show_grid: true,
            show_hover_crosshair: true,
            left_gutter: 46.0,
            x_ticks: 6,
            y_ticks: 8,
            background: Color32::BLACK,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CanvasState {
    pub view: Option<View2D>,
    pub home: Option<View2D>,
    /// Set when the user changed the view this frame (used for debounced re-rendering).
    pub last_interaction: f64,
}

impl CanvasState {
    pub fn reset(&mut self) {
        self.view = self.home;
    }
    /// Keep the user's zoom when the data identity is unchanged; refit otherwise.
    pub fn set_home(&mut self, home: View2D, refit: bool) {
        if refit || self.view.is_none() || self.home.is_none() {
            self.view = Some(home);
        }
        self.home = Some(home);
    }
}

pub struct CanvasOutput {
    pub response: Response,
    pub frame: PlotFrame,
    pub hover: Option<(f64, f64)>,
    pub changed_view: bool,
}

/// Lay out axes + data rect, run interaction, paint background/grid/axes, then call `draw`.
pub fn canvas(
    ui: &mut Ui,
    id: egui::Id,
    state: &mut CanvasState,
    p: &Palette,
    size: Vec2,
    opts: CanvasOpts<'_>,
    draw: impl FnOnce(&Painter, &PlotFrame),
) -> CanvasOutput {
    let (outer, _) = ui.allocate_exact_size(size, Sense::hover());
    let top_gutter = if opts.x_axis_top { 20.0 } else { 6.0 };
    let bottom_gutter = if opts.x_axis_top { 4.0 } else { 20.0 };
    let data_rect = Rect::from_min_max(
        pos2(outer.left() + opts.left_gutter, outer.top() + top_gutter),
        pos2(outer.right() - 6.0, outer.bottom() - bottom_gutter),
    );
    let response = ui.interact(data_rect, id, Sense::click_and_drag());

    let home = state.home.unwrap_or(View2D::new(0.0, 1.0, 0.0, 1.0));
    let mut view = state.view.filter(|v| v.is_valid()).unwrap_or(home);
    let mut frame = PlotFrame { rect: data_rect, view, y_down: opts.y_down };
    let mut changed = false;

    if response.hovered() {
        let (scroll, zoom, shift) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers.shift));
        if let Some(ptr) = response.hover_pos() {
            let (cx, cy) = frame.from_screen(ptr);
            let mut factor = zoom as f64;
            if scroll.y.abs() > 0.0 && !shift {
                factor *= (scroll.y as f64 * 0.0018).exp();
            }
            if (factor - 1.0).abs() > 1e-4 {
                let f = 1.0 / factor;
                if !opts.lock_x {
                    view.x0 = cx + (view.x0 - cx) * f;
                    view.x1 = cx + (view.x1 - cx) * f;
                }
                if !opts.lock_y {
                    view.y0 = cy + (view.y0 - cy) * f;
                    view.y1 = cy + (view.y1 - cy) * f;
                }
                changed = true;
            }
            let hscroll = if shift { scroll.y + scroll.x } else { scroll.x };
            if hscroll.abs() > 0.0 && !opts.lock_x {
                let dx = -hscroll as f64 / data_rect.width() as f64 * view.width();
                view.x0 += dx;
                view.x1 += dx;
                changed = true;
            }
        }
    }
    let pan_drag = response.dragged_by(egui::PointerButton::Middle)
        || (opts.primary_pans && response.dragged_by(egui::PointerButton::Primary));
    if pan_drag {
        let d = response.drag_delta();
        if !opts.lock_x {
            let dx = -d.x as f64 / data_rect.width() as f64 * view.width();
            view.x0 += dx;
            view.x1 += dx;
        }
        if !opts.lock_y {
            let sign = if opts.y_down { -1.0 } else { 1.0 };
            let dy = sign * d.y as f64 / data_rect.height() as f64 * view.height();
            view.y0 += dy;
            view.y1 += dy;
        }
        changed = true;
    }
    if response.double_clicked() {
        view = home;
        changed = true;
    }
    if changed {
        view = clamp_view(view, home);
        state.last_interaction = ui.input(|i| i.time);
    }
    state.view = Some(view);
    frame.view = view;

    let painter = ui.painter_at(outer);
    painter.rect_filled(data_rect, CornerRadius::ZERO, opts.background);

    let data_painter = painter.with_clip_rect(data_rect);
    draw(&data_painter, &frame);

    // Grid + ticks
    let tick_font = font_mono(10.5);
    let xt = nice_ticks(view.x0, view.x1, opts.x_ticks);
    let yt = nice_ticks(view.y0, view.y1, opts.y_ticks);
    let grid = Stroke::new(1.0, with_alpha(Color32::WHITE, 18));
    for &y in &yt {
        let sy = frame.y_to_screen(y);
        if opts.show_grid {
            data_painter.line_segment([pos2(data_rect.left(), sy), pos2(data_rect.right(), sy)], grid);
        }
        if let Some(label) = (opts.y_label)(y) {
            painter.text(
                pos2(data_rect.left() - 6.0, sy),
                egui::Align2::RIGHT_CENTER,
                label,
                tick_font.clone(),
                p.text_muted,
            );
            painter.line_segment([pos2(data_rect.left() - 3.0, sy), pos2(data_rect.left(), sy)], Stroke::new(1.0, p.border_strong));
        }
    }
    let mut last_label_right = f32::NEG_INFINITY;
    for &x in &xt {
        let sx = frame.x_to_screen(x);
        if opts.show_grid {
            data_painter.line_segment([pos2(sx, data_rect.top()), pos2(sx, data_rect.bottom())], grid);
        }
        if let Some(label) = (opts.x_label)(x) {
            let ly = if opts.x_axis_top { data_rect.top() - 4.0 } else { data_rect.bottom() + 4.0 };
            let align = if opts.x_axis_top { egui::Align2::CENTER_BOTTOM } else { egui::Align2::CENTER_TOP };
            let galley = painter.layout_no_wrap(label.clone(), tick_font.clone(), p.text_muted);
            let half = galley.size().x * 0.5;
            if sx - half > last_label_right + 6.0 && sx + half < outer.right() {
                painter.text(pos2(sx, ly), align, label, tick_font.clone(), p.text_muted);
                last_label_right = sx + half;
            }
        }
    }
    // Axis titles
    let title_font = font_mono(10.0);
    if !opts.y_title.is_empty() {
        painter.text(
            pos2(outer.left() + 2.0, data_rect.top() - if opts.x_axis_top { 6.0 } else { 0.0 }),
            if opts.x_axis_top { egui::Align2::LEFT_BOTTOM } else { egui::Align2::LEFT_TOP },
            &opts.y_title,
            title_font.clone(),
            p.text_faint,
        );
    }
    if !opts.x_title.is_empty() {
        let pos = if opts.x_axis_top {
            pos2(data_rect.right(), outer.top() + 1.0)
        } else {
            pos2(data_rect.right(), outer.bottom() - 1.0)
        };
        let align = if opts.x_axis_top { egui::Align2::RIGHT_TOP } else { egui::Align2::RIGHT_BOTTOM };
        let g = painter.layout_no_wrap(opts.x_title.clone(), title_font.clone(), p.text_faint);
        let bg = egui::Align2::anchor_size(align, pos, g.size()).expand(2.0);
        painter.rect_filled(bg, CornerRadius::same(2), p.panel);
        painter.text(pos, align, &opts.x_title, title_font, p.text_faint);
    }
    painter.rect_stroke(data_rect, CornerRadius::ZERO, Stroke::new(1.0, p.border_strong), StrokeKind::Outside);

    let hover = response.hover_pos().map(|pos| frame.from_screen(pos));
    if opts.show_hover_crosshair {
        if let Some(pos) = response.hover_pos() {
            let st = Stroke::new(1.0, with_alpha(p.crosshair, 110));
            data_painter.line_segment([pos2(data_rect.left(), pos.y), pos2(data_rect.right(), pos.y)], st);
            data_painter.line_segment([pos2(pos.x, data_rect.top()), pos2(pos.x, data_rect.bottom())], st);
        }
    }

    CanvasOutput { response, frame, hover, changed_view: changed }
}

fn clamp_view(mut v: View2D, home: View2D) -> View2D {
    let min_w = home.width() * 1e-3;
    let min_h = home.height() * 1e-3;
    if v.width() < min_w {
        let c = (v.x0 + v.x1) * 0.5;
        v.x0 = c - min_w * 0.5;
        v.x1 = c + min_w * 0.5;
    }
    if v.height() < min_h {
        let c = (v.y0 + v.y1) * 0.5;
        v.y0 = c - min_h * 0.5;
        v.y1 = c + min_h * 0.5;
    }
    let max_w = home.width() * 1.5;
    let max_h = home.height() * 1.5;
    if v.width() > max_w {
        let c = (v.x0 + v.x1) * 0.5;
        v.x0 = c - max_w * 0.5;
        v.x1 = c + max_w * 0.5;
    }
    if v.height() > max_h {
        let c = (v.y0 + v.y1) * 0.5;
        v.y0 = c - max_h * 0.5;
        v.y1 = c + max_h * 0.5;
    }
    let slack_x = home.width() * 0.25;
    let slack_y = home.height() * 0.25;
    if v.x1 < home.x0 + slack_x * 0.2 {
        let d = home.x0 + slack_x * 0.2 - v.x1;
        v.x0 += d;
        v.x1 += d;
    }
    if v.x0 > home.x1 - slack_x * 0.2 {
        let d = v.x0 - (home.x1 - slack_x * 0.2);
        v.x0 -= d;
        v.x1 -= d;
    }
    if v.y1 < home.y0 + slack_y * 0.2 {
        let d = home.y0 + slack_y * 0.2 - v.y1;
        v.y0 += d;
        v.y1 += d;
    }
    if v.y0 > home.y1 - slack_y * 0.2 {
        let d = v.y0 - (home.y1 - slack_y * 0.2);
        v.y0 -= d;
        v.y1 -= d;
    }
    v
}

/// Amplitude at the given clip percentile (same sampling strategy as the L3 renderer).
pub fn clip_amplitude(traces: &[Trace], percentile: f32) -> f32 {
    let mut vals = Vec::new();
    let step_tr = (traces.len() / 64).max(1);
    for tr in traces.iter().step_by(step_tr) {
        if !tr.is_live() {
            continue;
        }
        let step_s = (tr.samples.len() / 256).max(1);
        for &s in tr.samples.iter().step_by(step_s) {
            if s.is_finite() && s != 0.0 {
                vals.push(s.abs());
            }
        }
    }
    if vals.is_empty() {
        return 1.0;
    }
    vals.sort_by(|a, b| a.total_cmp(b));
    let idx = ((percentile.clamp(50.0, 100.0) / 100.0) * (vals.len() - 1) as f32).round() as usize;
    vals[idx.min(vals.len() - 1)].max(1e-12)
}

/// Vector wiggle traces with positive variable-area fill. `first_x` is the data x of trace 0's
/// column start (trace i is centred at `first_x + i + 0.5`). Only visible traces are tessellated.
#[allow(clippy::too_many_arguments)]
pub fn draw_wiggles(
    painter: &Painter,
    frame: &PlotFrame,
    traces: &[Trace],
    first_x: f64,
    clip: f32,
    gain: f32,
    line: Color32,
    fill: Option<Color32>,
) {
    if traces.is_empty() {
        return;
    }
    let dt = traces[0].dt_ms() as f64;
    let px_per_trace = frame.px_per_x();
    if px_per_trace < 2.5 {
        return;
    }
    let i_lo = ((frame.view.x0 - first_x).floor().max(0.0)) as usize;
    let i_hi = (((frame.view.x1 - first_x).ceil()).max(0.0) as usize).min(traces.len());
    if i_hi <= i_lo || i_hi - i_lo > 400 {
        return;
    }
    let s_lo = ((frame.view.y0 / dt).floor().max(0.0)) as usize;
    let rows_px = frame.rect.height().max(1.0);
    let span_samples = (frame.view.height() / dt).max(1.0);
    let stride = ((span_samples / rows_px as f64).floor() as usize).max(1);
    let amp_px = px_per_trace * 0.5 * gain / clip.max(1e-12);
    let max_dev = px_per_trace * 1.6;
    let stroke = Stroke::new(if px_per_trace > 10.0 { 1.0 } else { 0.7 }, line);
    let mut mesh = Mesh::default();

    for (ii, tr) in traces.iter().enumerate().take(i_hi).skip(i_lo) {
        if !tr.is_live() {
            continue;
        }
        let x0 = frame.x_to_screen(first_x + ii as f64 + 0.5);
        let n = tr.samples.len();
        let s_end = (((frame.view.y1 / dt).ceil() as usize) + 1).min(n);
        if s_lo >= s_end {
            continue;
        }
        let mut pts: Vec<Pos2> = Vec::with_capacity((s_end - s_lo) / stride + 2);
        let mut s = s_lo;
        while s < s_end {
            let a = (tr.samples[s] * amp_px).clamp(-max_dev, max_dev);
            pts.push(pos2(x0 + a, frame.y_to_screen(s as f64 * dt)));
            s += stride;
        }
        if let Some(fc) = fill {
            for w in pts.windows(2) {
                let (a, b) = (w[0], w[1]);
                let (da, db) = (a.x - x0, b.x - x0);
                if da <= 0.0 && db <= 0.0 {
                    continue;
                }
                let base = mesh.vertices.len() as u32;
                let (ax, bx) = (x0 + da.max(0.0), x0 + db.max(0.0));
                mesh.colored_vertex(pos2(x0, a.y), fc);
                mesh.colored_vertex(pos2(ax, a.y), fc);
                mesh.colored_vertex(pos2(bx, b.y), fc);
                mesh.colored_vertex(pos2(x0, b.y), fc);
                mesh.add_triangle(base, base + 1, base + 2);
                mesh.add_triangle(base, base + 2, base + 3);
            }
        }
        painter.add(Shape::line(pts, stroke));
    }
    if !mesh.is_empty() {
        painter.add(Shape::mesh(mesh));
    }
}

/// Small text chip drawn over a canvas (e.g. "CDP 2304 · fold 48").
pub fn badge(painter: &Painter, pos: Pos2, text: &str, p: &Palette, align: egui::Align2) {
    let font = font_mono(10.5);
    let g = painter.layout_no_wrap(text.to_string(), font, p.text);
    let r = align.anchor_size(pos, g.size() + vec2(10.0, 4.0));
    painter.rect(r, CornerRadius::same(3), with_alpha(p.chrome, 215), Stroke::new(1.0, p.border_strong), StrokeKind::Inside);
    painter.galley(r.min + vec2(5.0, 2.0), g, p.text);
}

/// Centered empty-state message inside a rect.
pub fn empty_state(ui: &mut Ui, p: &Palette, icon: &str, title: &str, hint: &str) {
    let rect = ui.available_rect_before_wrap();
    let painter = ui.painter();
    let c = rect.center();
    painter.text(c - vec2(0.0, 26.0), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(30.0), p.text_faint);
    painter.text(c + vec2(0.0, 4.0), egui::Align2::CENTER_CENTER, title, crate::theme::font_display(15.0), p.text_muted);
    painter.text(c + vec2(0.0, 24.0), egui::Align2::CENTER_CENTER, hint, egui::FontId::proportional(12.0), p.text_faint);
    ui.allocate_rect(rect, Sense::hover());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_are_nice_and_cover_range() {
        let t = nice_ticks(0.0, 3500.0, 7);
        assert_eq!(t.first().copied(), Some(0.0));
        assert!(t.windows(2).all(|w| ((w[1] - w[0]) - 500.0).abs() < 1e-9));
        assert!(nice_ticks(5.0, 5.0, 5).is_empty());
    }

    #[test]
    fn frame_round_trips_both_orientations() {
        for y_down in [true, false] {
            let f = PlotFrame {
                rect: Rect::from_min_size(pos2(10.0, 20.0), vec2(400.0, 300.0)),
                view: View2D::new(0.0, 96.0, 0.0, 4000.0),
                y_down,
            };
            let p = f.to_screen(48.0, 1000.0);
            let (x, y) = f.from_screen(p);
            assert!((x - 48.0).abs() < 1e-3 && (y - 1000.0).abs() < 1e-2);
        }
    }
}
