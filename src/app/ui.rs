//! egui layout of the main window and the settings window.

use std::sync::atomic::{AtomicBool, Ordering};

use egui::{Align, Color32, CornerRadius, Layout, Margin, RichText, Sense, Stroke, Ui, vec2};

use super::theme::{self, BODY, SECONDARY, TOOLBAR_H, TRAFFIC_LIGHTS_W};
use super::{Cmd, Meter, State};
use crate::compose::{self, Corner, Place, Scene, Shape};
use crate::settings::Settings;
use crate::voice::Verdict;
use crate::settings::{FPS, RESOLUTIONS};
use crate::writer::{AudioLayout, Codec};

const RED: Color32 = Color32::from_rgb(0xff, 0x3b, 0x30);
const AMBER: Color32 = Color32::from_rgb(0xff, 0x9f, 0x0a);
const GREEN: Color32 = Color32::from_rgb(0x34, 0xc7, 0x59);

/// Colour of a mic verdict: green when good, amber for something to look
/// at, red when the recording would be hard to fix.
fn verdict_color(v: Verdict, p: &theme::Palette) -> Color32 {
    if v.is_bad() {
        RED
    } else if v.is_warning() {
        AMBER
    } else if v == Verdict::Good {
        GREEN
    } else {
        p.tertiary
    }
}

/// The mic's verdict as a pill beside the meters; its advice on hover.
fn mic_badge(ui: &mut Ui, st: &State) {
    let Some(r) = st.mic.filter(|_| st.settings.mic_enabled) else { return };
    let p = theme::pal(ui);
    let v = r.verdict;
    let col = verdict_color(v, &p);
    let text = match r.speech_peak_db {
        Some(db) if v != Verdict::Silent => format!("{}  {db:.0} dB", v.label()),
        _ => v.label().to_string(),
    };
    let mut advice = v.advice().to_string();
    if r.speech_peak_db.is_some() {
        advice += "\n\nThe number is your typical speech peak. Aim for about -12 to -6 dB.";
    }
    egui::Frame::new()
        .fill(col.gamma_multiply(0.18))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 4.0, col);
                ui.label(RichText::new(text).size(SECONDARY).color(p.text));
            });
        })
        .response
        .on_hover_text(advice);
}

/// A problem that would spoil the recording, across the top of the preview
/// once it has lasted two seconds.
fn mic_banner(ui: &Ui, st: &State, preview: egui::Rect) {
    let (Some(r), Some(since)) = (st.mic, st.mic_problem_since) else { return };
    if !r.verdict.is_bad() || since.elapsed() < std::time::Duration::from_secs(2) || !st.settings.mic_enabled {
        return;
    }
    let text = format!("Mic: {}. {}", r.verdict.label(), r.verdict.advice());
    let font = egui::FontId::proportional(BODY);
    let galley = ui.painter().layout(text, font, Color32::WHITE, preview.width() - 48.0);
    let size = galley.size() + vec2(24.0, 14.0);
    let banner = egui::Rect::from_min_size(egui::pos2(preview.center().x - size.x / 2.0, preview.top() + 12.0), size);
    ui.painter().rect_filled(banner, CornerRadius::same(8), RED.gamma_multiply(0.92));
    ui.painter().galley(banner.min + vec2(12.0, 7.0), galley, Color32::WHITE);
}
const SIDEBAR_W: f32 = 290.0;

static DRAG: AtomicBool = AtomicBool::new(false);

/// Whether the toolbar asked to drag the window this frame.
pub fn take_drag_request() -> bool {
    DRAG.swap(false, Ordering::Relaxed)
}

/// A path with the home folder shown as `~`.
fn tilde(p: &std::path::Path) -> String {
    match std::env::var_os("HOME").map(std::path::PathBuf::from) {
        Some(h) if p.starts_with(&h) => format!("~/{}", p.strip_prefix(&h).unwrap().display()),
        _ => p.display().to_string(),
    }
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).font(theme::semibold(SECONDARY)).color(theme::pal(ui).secondary));
    ui.add_space(2.0);
}

/// Toolbar strip: empty space drags the window.
fn toolbar(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let rect = ui.max_rect();
    let bg = ui.interact(rect, ui.id().with("drag"), Sense::click_and_drag());
    if bg.drag_started() {
        DRAG.store(true, Ordering::Relaxed);
    }
    ui.horizontal_centered(|ui| {
        ui.add_space(TRAFFIC_LIGHTS_W);
        add(ui);
    });
    let p = theme::pal(ui);
    ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, p.separator));
}

fn pill(ui: &mut Ui, text: &str, fill: Color32, fg: Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), theme::semibold(SECONDARY), fg);
    let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(16.0, 6.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(9), fill);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, fg);
}

pub fn main_window(ui: &mut Ui, st: &mut State) {
    let p = theme::pal(ui);
    let recording = st.recording.is_some();
    egui::Panel::top("toolbar")
        .exact_size(TOOLBAR_H)
        .frame(egui::Frame::NONE.fill(p.window).inner_margin(Margin::symmetric(12, 0)))
        .show(ui, |ui| {
            toolbar(ui, |ui| {
                ui.label(RichText::new("Rec").font(theme::semibold(BODY)));
                ui.add_space(6.0);
                if st.finishing {
                    pill(ui, "SAVING…", p.control, p.secondary);
                } else if st.is_paused() {
                    pill(ui, &format!("PAUSED  {}", st.clock()), Color32::from_rgb(0xff, 0x95, 0x00), Color32::WHITE);
                } else if recording {
                    pill(ui, &format!("● REC  {}", st.clock()), RED, Color32::WHITE);
                } else if st.preview.is_some() {
                    pill(ui, "PREVIEW", p.control, p.secondary);
                }
                if st.stream_status.as_ref().is_some_and(|s| s.live) {
                    ui.add_space(4.0);
                    pill(ui, "LIVE", RED, Color32::WHITE);
                }
                if let Some((text, at)) = &st.notice
                    && at.elapsed().as_secs_f32() < 4.0
                {
                    ui.add_space(6.0);
                    // Cut to fit, leaving room for the buttons on the right.
                    let w = (ui.available_width() - 160.0).max(40.0);
                    ui.allocate_ui(vec2(w, ui.available_height()), |ui| {
                        ui.add(egui::Label::new(RichText::new(text).size(SECONDARY).color(p.secondary)).truncate()).on_hover_text(text);
                    });
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Settings…").on_hover_text("Output, codec, resolution, audio (⌘,)").clicked() {
                        st.cmds.push(Cmd::ShowSettings);
                    }
                    if !st.hotkey_ok {
                        ui.label(RichText::new("⇧⌘R unavailable").color(p.secondary));
                    }
                });
            });
        });
    egui::Panel::bottom("controls")
        .exact_size(68.0)
        .frame(egui::Frame::NONE.fill(p.window).inner_margin(Margin::symmetric(16, 10)))
        .show(ui, |ui| controls(ui, st));
    egui::Panel::right("sources")
        .exact_size(SIDEBAR_W)
        .frame(egui::Frame::NONE.fill(p.sidebar).inner_margin(Margin { left: 14, right: 4, top: 6, bottom: 6 }))
        .show(ui, |ui| {
            // Lay the contents out in a child of exactly the panel's
            // size: a too-wide widget (a long device name) would otherwise
            // widen the panel past the window edge, under the preview.
            let rect = ui.max_rect();
            let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)));
            inner.set_clip_rect(rect.intersect(ui.clip_rect()));
            egui::ScrollArea::vertical().auto_shrink(false).show(&mut inner, |ui| {
                ui.set_max_width(SIDEBAR_W - 32.0);
                sources(ui, st)
            });
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(p.window).inner_margin(Margin::same(16)))
        .show(ui, |ui| preview(ui, st));
    // Keyboard: ⌘, settings, ⌘W close (handled as hide by the app), space
    // does nothing (avoid accidental recordings).
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma)) {
        st.cmds.push(Cmd::ShowSettings);
    }
}

fn preview(ui: &mut Ui, st: &mut State) {
    let p = theme::pal(ui);
    if let Some(err) = st.error.clone() {
        egui::Frame::NONE
            .fill(Color32::from_rgba_unmultiplied(0xff, 0x3b, 0x30, 30))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(err).color(p.text));
                    if st.permissions_missing && ui.button("Request Permissions").clicked() {
                        crate::tcc::request_all(true, true, true, std::time::Duration::from_secs(0));
                    }
                    if ui.button("Dismiss").clicked() {
                        st.error = None;
                    }
                });
            });
        ui.add_space(10.0);
    }
    let avail = ui.available_size();
    let ppp = ui.ctx().pixels_per_point();
    // The scaler renders frames at exactly this many pixels (aspect kept).
    super::preview::set_target((avail.x * ppp).floor() as u32, (avail.y * ppp).floor() as u32);
    let aspect = st.preview_aspect.max(0.1);
    let mut size = vec2(avail.x, avail.x / aspect);
    if size.y > avail.y {
        size = vec2(avail.y * aspect, avail.y);
    }
    // Show the frame at its own pixel size when it fits (1:1, no second
    // scaling), e.g. always on Retina, except in low-power mode.
    let px = vec2(st.preview_px[0] as f32, st.preview_px[1] as f32) / ppp;
    if px.x > 0.0 && px.x <= avail.x + 0.5 && px.y <= avail.y + 0.5 && !st.settings.preview_low_power {
        size = px;
    }
    ui.vertical_centered(|ui| {
        ui.add_space(((avail.y - size.y) / 2.0).max(0.0));
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        // Snap to the pixel grid so texels land on pixels.
        let rect = egui::Rect::from_min_size(
            egui::pos2((rect.min.x * ppp).round() / ppp, (rect.min.y * ppp).round() / ppp),
            rect.size(),
        );
        // REC_DEBUG_PREVIEW: where the preview is, in window pixels (for
        // comparing it against a screenshot of the screen).
        if std::env::var_os("REC_DEBUG_PREVIEW").is_some() {
            eprintln!(
                "rec: preview rect {} {} {} {}",
                (rect.min.x * ppp).round(),
                (rect.min.y * ppp).round(),
                (rect.width() * ppp).round(),
                (rect.height() * ppp).round()
            );
        }
        // Square and edge to edge: rounded corners would show the backdrop
        // (black corners), and the frame already has the source's shape.
        let r = CornerRadius::ZERO;
        let live = st.recording.is_some() || st.preview.is_some();
        match &st.preview_tex {
            Some(id) if live => {
                egui::Image::new((*id, size)).paint_at(ui, rect);
            }
            _ => {
                ui.painter().rect_filled(rect, CornerRadius::same(10), theme::pal(ui).control);
                let msg = if st.finishing { "Saving…" } else { "No preview" };
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, msg, egui::FontId::proportional(BODY), Color32::GRAY);
            }
        }
        if st.recording.is_some() {
            let border = if st.is_paused() { Color32::from_rgb(0xff, 0x95, 0x00) } else { RED };
            // Around the picture, not over its edges.
            ui.painter().rect_stroke(rect, r, Stroke::new(2.0, border), egui::StrokeKind::Outside);
        }
        if live && st.settings.scene.has_overlay() {
            drag_overlay(ui, st, rect);
        }
        mic_banner(ui, st, rect);
    });
}

/// Output-pixel frame the overlay geometry is computed in (any size with
/// the preview's aspect ratio; positions are fractions of it).
fn frame_size(st: &State) -> (f64, f64) {
    (3840.0, (3840.0 / st.preview_aspect.max(0.1) as f64).round())
}

fn overlay_ui_rect(st: &State, spec: &compose::PipSpec, preview: egui::Rect) -> egui::Rect {
    let (w, h) = frame_size(st);
    let [x, y, ow, oh] = compose::overlay_rect(spec, w, h, st.cam_aspect);
    let (sx, sy) = (preview.width() as f64 / w, preview.height() as f64 / h);
    egui::Rect::from_min_size(
        preview.min + vec2((x * sx) as f32, (y * sy) as f32),
        vec2((ow * sx) as f32, (oh * sy) as f32),
    )
}

/// The overlay edges a drag moves; none means it moves the whole overlay.
#[derive(Clone, Copy, Default, PartialEq)]
struct Edges {
    l: bool,
    r: bool,
    t: bool,
    b: bool,
}

impl Edges {
    /// Edges within `grip` points of `p`.
    fn at(ov: egui::Rect, p: egui::Pos2, grip: f32) -> Edges {
        Edges {
            l: (p.x - ov.left()).abs() <= grip,
            r: (p.x - ov.right()).abs() <= grip,
            t: (p.y - ov.top()).abs() <= grip,
            b: (p.y - ov.bottom()).abs() <= grip,
        }
    }

    fn cursor(self) -> egui::CursorIcon {
        use egui::CursorIcon as C;
        match (self.l || self.r, self.t || self.b) {
            (true, true) if self.l == self.t => C::ResizeNwSe,
            (true, true) => C::ResizeNeSw,
            (true, false) => C::ResizeHorizontal,
            (false, true) => C::ResizeVertical,
            _ => C::Default,
        }
    }
}

/// Move the dragged edges of the overlay by `d`, keeping the others in
/// place. Width and height change independently, for every shape.
fn resize_overlay(st: &mut State, preview: egui::Rect, ov: egui::Rect, e: Edges, d: egui::Vec2) {
    let (pw, ph) = (preview.width(), preview.height());
    let w = (ov.width() + if e.l { -d.x } else if e.r { d.x } else { 0.0 }).clamp(0.05 * pw, 0.9 * pw);
    let h = (ov.height() + if e.t { -d.y } else if e.b { d.y } else { 0.0 }).clamp(0.05 * ph, ph);
    // Keep the edges opposite the dragged ones where they are.
    let x0 = if e.l { ov.right() - w } else { ov.left() };
    let y0 = if e.t { ov.bottom() - h } else { ov.top() };
    let c = egui::Rect::from_min_size(egui::pos2(x0, y0), vec2(w, h)).center();
    let s = &mut st.settings;
    s.pip_size = (w / pw) as f64;
    s.pip_height = Some((h / ph) as f64);
    s.pip_place = Place::At([((c.x - preview.min.x) / pw) as f64, ((c.y - preview.min.y) / ph) as f64]);
}

/// Move the camera picture inside the overlay by `d` points.
fn pan_camera(st: &mut State, ov: egui::Rect, d: egui::Vec2) {
    let s = &mut st.settings;
    let (w, h) = (ov.width() as f64, ov.height() as f64);
    let (k, _, _) = compose::cam_place(st.cam_aspect, 1.0, w, h, s.pip_zoom, s.pip_pan);
    // How far the picture overflows the overlay each way, in points.
    let (ex, ey) = (st.cam_aspect * k - w, k - h);
    if ex > 0.5 {
        s.pip_pan[0] = (s.pip_pan[0] - d.x as f64 / ex).clamp(0.0, 1.0);
    }
    if ey > 0.5 {
        s.pip_pan[1] = (s.pip_pan[1] - d.y as f64 / ey).clamp(0.0, 1.0);
    }
}

/// What a drag on the overlay does.
#[derive(Clone, Copy, PartialEq)]
enum DragMode {
    Move,
    Resize(Edges),
    /// Option-drag inside the overlay: reframe the camera picture.
    Pan,
}

impl DragMode {
    fn at(ov: egui::Rect, p: egui::Pos2, grip: f32, alt: bool) -> DragMode {
        let e = Edges::at(ov, p, grip);
        if e != Edges::default() {
            DragMode::Resize(e)
        } else if alt {
            DragMode::Pan
        } else {
            DragMode::Move
        }
    }
}

/// Drag the camera overlay in the preview to move it, drag its edges and
/// corners to resize it, Option-drag inside it to reframe the camera, and
/// scroll or pinch over it to zoom. Changes apply live (in the preview and
/// the recording), and it snaps to a corner when released near one.
fn drag_overlay(ui: &mut Ui, st: &mut State, preview: egui::Rect) {
    const SNAP: f64 = 0.05;
    // Resize handles reach this far either side of an edge, in points.
    const GRIP: f32 = 6.0;
    let spec = st.settings.pip_spec();
    let ov = overlay_ui_rect(st, &spec, preview);
    let id = ui.id().with("pip-drag");
    let resp = ui.interact(ov.expand(GRIP), id, Sense::drag());
    let alt = ui.input(|i| i.modifiers.alt);
    if resp.drag_started()
        && let Some(p) = ui.input(|i| i.pointer.press_origin())
    {
        ui.data_mut(|d| d.insert_temp(id, DragMode::at(ov, p, GRIP, alt)));
    }
    let mode = if resp.dragged() || resp.drag_stopped() {
        ui.data(|d| d.get_temp::<DragMode>(id)).unwrap_or(DragMode::Move)
    } else {
        resp.hover_pos().map(|p| DragMode::at(ov, p, GRIP, alt)).unwrap_or(DragMode::Move)
    };
    let accent = theme::accent();
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(match mode {
            DragMode::Resize(e) => e.cursor(),
            DragMode::Pan => egui::CursorIcon::Move,
            DragMode::Move if resp.dragged() => egui::CursorIcon::Grabbing,
            DragMode::Move => egui::CursorIcon::Grab,
        });
        ui.painter().rect_stroke(ov, CornerRadius::same(4), Stroke::new(1.5, accent), egui::StrokeKind::Outside);
        // Corner handles show the overlay can be resized.
        for c in [ov.left_top(), ov.right_top(), ov.left_bottom(), ov.right_bottom()] {
            ui.painter().circle(c, 4.0, Color32::WHITE, Stroke::new(1.5, accent));
        }
    }
    if resp.hovered() && !resp.dragged() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let zoom = (st.settings.pip_zoom * pinch as f64 * (scroll as f64 * 0.005).exp()).clamp(1.0, 4.0);
        if zoom != st.settings.pip_zoom {
            st.settings.pip_zoom = zoom;
            st.apply_pip();
            let _ = st.settings.save();
        }
    }
    if resp.dragged() {
        let d = resp.drag_delta();
        match mode {
            DragMode::Move => {
                let (cx, cy) = ((ov.center().x - preview.min.x) / preview.width(), (ov.center().y - preview.min.y) / preview.height());
                let at = [(cx + d.x / preview.width()).clamp(0.0, 1.0) as f64, (cy + d.y / preview.height()).clamp(0.0, 1.0) as f64];
                st.settings.pip_place = Place::At(at);
            }
            DragMode::Resize(e) => resize_overlay(st, preview, ov, e, d),
            DragMode::Pan => pan_camera(st, ov, d),
        }
        // Show where it would snap.
        let (w, h) = frame_size(st);
        if mode != DragMode::Pan
            && let Some(c) = compose::snap_corner(&st.settings.pip_spec(), w, h, st.cam_aspect, SNAP)
        {
            let ghost = overlay_ui_rect(st, &compose::PipSpec { place: Place::Corner(c), ..st.settings.pip_spec() }, preview);
            ui.painter().rect_stroke(ghost, CornerRadius::same(4), Stroke::new(1.0, accent.gamma_multiply(0.7)), egui::StrokeKind::Outside);
        }
        st.apply_pip();
    }
    if resp.drag_stopped() {
        let (w, h) = frame_size(st);
        if mode != DragMode::Pan
            && let Some(c) = compose::snap_corner(&st.settings.pip_spec(), w, h, st.cam_aspect, SNAP)
        {
            st.settings.pip_place = Place::Corner(c);
        }
        st.apply_pip();
        let _ = st.settings.save();
    }
}

/// A small icon of `shape` (with the current roundness).
fn shape_icon(ui: &Ui, rect: egui::Rect, shape: Shape, roundness: f64, color: Color32) {
    let c = rect.center();
    let (w, h) = if shape.is_square() { (16.0, 16.0) } else { (22.0, 13.0) };
    let r = egui::Rect::from_center_size(c, vec2(w, h));
    let painter = ui.painter();
    let stroke = Stroke::new(1.5, color);
    match shape {
        Shape::Circle => {
            painter.circle_stroke(c, 8.0, stroke);
        }
        Shape::Squircle => {
            let n = compose::squircle_exponent(roundness);
            let pts: Vec<egui::Pos2> = (0..64)
                .map(|i| {
                    let t = i as f64 / 64.0 * std::f64::consts::TAU;
                    let (ct, s) = (t.cos(), t.sin());
                    let x = ct.signum() * ct.abs().powf(2.0 / n) * 8.0;
                    let y = s.signum() * s.abs().powf(2.0 / n) * 8.0;
                    c + vec2(x as f32, y as f32)
                })
                .collect();
            painter.add(egui::Shape::closed_line(pts, stroke));
        }
        _ => {
            let rad = match shape {
                Shape::Pill => h / 2.0,
                Shape::Rect => 0.0,
                _ => roundness as f32 * h / 2.0,
            };
            painter.rect_stroke(r, CornerRadius::same(rad.round() as u8), stroke, egui::StrokeKind::Middle);
        }
    }
}

/// The overlay height (fraction of the frame height) while it is automatic:
/// as wide as it is tall for square shapes, else the camera's shape.
fn auto_height(st: &State, s: &Settings) -> f64 {
    let aspect = if s.pip_shape.is_square() { 1.0 } else { st.cam_aspect.max(0.1) };
    s.pip_size * st.preview_aspect as f64 / aspect
}

/// Camera overlay look and position. Live: also while recording.
/// `auto_height` is the height (fraction of the frame) shown while the
/// height is automatic.
fn overlay_controls(ui: &mut Ui, s: &mut Settings, auto_height: f64) {
    let p = theme::pal(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let bw = ((ui.available_width() - 16.0) / 5.0).max(30.0);
        for shape in Shape::PICKER {
            let sel = s.pip_shape == shape;
            let (rect, resp) = ui.allocate_exact_size(vec2(bw, 30.0), Sense::click());
            let fill = if sel { theme::accent() } else if resp.hovered() { p.hover } else { p.control };
            ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
            shape_icon(ui, rect, shape, s.pip_roundness, if sel { Color32::WHITE } else { p.text });
            if resp.on_hover_text(shape.label()).clicked() {
                s.pip_shape = shape;
            }
        }
    });
    ui.add_space(4.0);
    // Label (64) + slider + value box (about 52) + spacing must fit.
    let slider_w = (ui.available_width() - 64.0 - 60.0 - 40.0).max(60.0);
    let row = |ui: &mut Ui, label: &str, add: &mut dyn FnMut(&mut Ui)| {
        ui.horizontal(|ui| {
            ui.add_sized(vec2(64.0, 18.0), egui::Label::new(RichText::new(label).size(SECONDARY).color(p.secondary)));
            add(ui);
        });
    };
    ui.spacing_mut().slider_width = slider_w;
    row(ui, "Width", &mut |ui| {
        let mut pct = s.pip_size * 100.0;
        if ui.add(egui::Slider::new(&mut pct, 5.0..=90.0).suffix(" %").integer()).changed() {
            s.pip_size = pct / 100.0;
        }
    });
    row(ui, "Height", &mut |ui| {
        // Narrower slider: the Auto button shares the row.
        ui.spacing_mut().slider_width = slider_w - 40.0;
        let mut pct = s.pip_height.unwrap_or(auto_height.min(1.0)) * 100.0;
        if ui.add(egui::Slider::new(&mut pct, 5.0..=100.0).suffix(" %").integer()).changed() {
            s.pip_height = Some(pct / 100.0);
        }
        let auto = s.pip_height.is_none();
        let tip = if s.pip_shape.is_square() { "Keep it as tall as it is wide" } else { "Follow the camera's shape" };
        if ui.add(egui::Button::selectable(auto, "Auto")).on_hover_text(tip).clicked() {
            s.pip_height = None;
        }
    });
    row(ui, "Zoom", &mut |ui| {
        // Narrower slider: the Center button shares the row.
        ui.spacing_mut().slider_width = slider_w - 50.0;
        let mut pct = s.pip_zoom * 100.0;
        if ui.add(egui::Slider::new(&mut pct, 100.0..=400.0).suffix(" %").integer()).changed() {
            s.pip_zoom = pct / 100.0;
        }
        let centred = s.pip_pan == [0.5, 0.5];
        if ui.add_enabled(!centred, egui::Button::new("Center")).on_hover_text("Center the camera in the overlay").clicked() {
            s.pip_pan = [0.5, 0.5];
        }
    });
    row(ui, "Roundness", &mut |ui| {
        let mut pct = s.pip_roundness * 100.0;
        if ui.add_enabled(s.pip_shape.uses_roundness(), egui::Slider::new(&mut pct, 0.0..=100.0).suffix(" %").integer()).changed() {
            s.pip_roundness = pct / 100.0;
        }
    });
    row(ui, "Border", &mut |ui| {
        // Narrower slider: the color swatch shares the row.
        ui.spacing_mut().slider_width = slider_w - 30.0;
        ui.add(egui::Slider::new(&mut s.pip_border, 0.0..=20.0).suffix(" px").integer());
        ui.add_enabled_ui(s.pip_border > 0.0, |ui| {
            ui.spacing_mut().interact_size = vec2(22.0, 18.0);
            ui.color_edit_button_srgb(&mut s.pip_border_color);
        });
    });
    row(ui, "Position", &mut |ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for (c, label, tip) in [
            (Corner::TopLeft, "↖", "Top left"),
            (Corner::TopRight, "↗", "Top right"),
            (Corner::BottomLeft, "↙", "Bottom left"),
            (Corner::BottomRight, "↘", "Bottom right"),
        ] {
            let sel = s.pip_place == Place::Corner(c);
            if ui.add(egui::Button::selectable(sel, label).min_size(vec2(26.0, 22.0))).on_hover_text(tip).clicked() {
                s.pip_place = Place::Corner(c);
            }
        }
        if matches!(s.pip_place, Place::At(_)) {
            ui.label(RichText::new("custom").size(SECONDARY).color(p.secondary));
        }
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut s.pip_shadow, "Shadow");
        ui.checkbox(&mut s.mirror, "Mirror");
    });
    ui.label(
        RichText::new(
            "In the preview: drag the camera to move it (it snaps to corners), drag its edges to resize it, \
             scroll over it to zoom, and Option-drag inside it to reframe.",
        )
            .size(SECONDARY)
            .color(p.secondary),
    );
}

/// Presenter background: a color, or an image covering the frame.
fn background_controls(ui: &mut Ui, s: &mut Settings) {
    let p = theme::pal(ui);
    ui.horizontal(|ui| {
        ui.add_sized(vec2(64.0, 18.0), egui::Label::new(RichText::new("Color").size(SECONDARY).color(p.secondary)));
        ui.spacing_mut().interact_size = vec2(36.0, 20.0);
        ui.color_edit_button_srgb(&mut s.bg_color);
        if ui.button("Image…").on_hover_text("Use a picture instead of the color").clicked()
            && let Some(f) = rfd::FileDialog::new()
                .add_filter("Images", &["jpg", "jpeg", "png", "heic", "tif", "tiff", "webp", "gif", "bmp"])
                .pick_file()
        {
            s.bg_image = Some(f);
        }
        if s.bg_image.is_some() && ui.button("Remove").on_hover_text("Go back to the color").clicked() {
            s.bg_image = None;
        }
    });
    if let Some(f) = &s.bg_image {
        let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        ui.add(egui::Label::new(RichText::new(name).size(SECONDARY).color(p.secondary)).truncate()).on_hover_text(tilde(f));
    }
    ui.label(
        RichText::new("Your screen sits in a frame beside the camera. Drag the camera to the other side and the screen follows.")
            .size(SECONDARY)
            .color(p.secondary),
    );
}

fn controls(ui: &mut Ui, st: &mut State) {
    let p = theme::pal(ui);
    let recording = st.recording.is_some();
    ui.horizontal_centered(|ui| {
        // Record / stop: a round button like a camera's.
        let (rect, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
        let c = rect.center();
        let painter = ui.painter();
        painter.circle_stroke(c, 21.0, Stroke::new(3.0, if resp.hovered() { p.text } else { p.secondary }));
        if st.finishing {
            painter.circle_filled(c, 15.0, p.tertiary);
        } else if recording {
            painter.rect_filled(egui::Rect::from_center_size(c, vec2(18.0, 18.0)), CornerRadius::same(4), RED);
        } else {
            painter.circle_filled(c, 16.0, RED);
        }
        let tip = if recording { "Stop recording (⇧⌘R)" } else { "Start recording (⇧⌘R)" };
        if resp.on_hover_text(tip).clicked() && !st.finishing {
            st.cmds.push(Cmd::ToggleRecording);
        }
        ui.add_space(6.0);
        let paused = st.is_paused();
        if ui
            .add_enabled(recording, egui::Button::new(if paused { "Resume" } else { "Pause" }).min_size(vec2(72.0, 26.0)))
            .clicked()
        {
            st.cmds.push(Cmd::TogglePause);
        }
        ui.add_space(10.0);
        let clock = if recording { st.clock() } else { "00:00".into() };
        let color = if paused { p.secondary } else if recording { RED } else { p.tertiary };
        ui.label(RichText::new(clock).font(egui::FontId::monospace(22.0)).color(color));
        ui.add_space(16.0);
        ui.vertical(|ui| {
            ui.add_space(4.0);
            meter(ui, "System", st.meters[0], st.settings.system_audio);
            meter(ui, "Mic", st.meters[1], st.settings.mic_enabled);
        });
        ui.add_space(8.0);
        mic_badge(ui, st);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("Open Folder").clicked() {
                st.cmds.push(Cmd::OpenFolder);
            }
            if st.last_file.is_some() && ui.button("Reveal Last").clicked() {
                st.cmds.push(Cmd::RevealLast);
            }
        });
    });
}

/// Horizontal peak meter, -60..0 dBFS, green/yellow/red with a peak tick.
fn meter(ui: &mut Ui, label: &str, m: Meter, on: bool) {
    let p = theme::pal(ui);
    ui.horizontal(|ui| {
        ui.add_sized(vec2(44.0, 14.0), egui::Label::new(RichText::new(label).size(SECONDARY).color(p.secondary)));
        let (rect, _) = ui.allocate_exact_size(vec2(220.0, 8.0), Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(3), p.control);
        if !on {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, "off", egui::FontId::proportional(9.0), p.tertiary);
            return;
        }
        let frac = |db: f32| ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        let x = |f: f32| rect.left() + rect.width() * f;
        let level = frac(m.db);
        for (lo, hi, col) in [
            (0.0, frac(-18.0), Color32::from_rgb(0x34, 0xc7, 0x59)),
            (frac(-18.0), frac(-6.0), Color32::from_rgb(0xff, 0xcc, 0x00)),
            (frac(-6.0), 1.0, RED),
        ] {
            let hi = hi.min(level);
            if hi > lo {
                let r = egui::Rect::from_x_y_ranges(x(lo)..=x(hi), rect.y_range());
                painter.rect_filled(r, CornerRadius::same(2), col);
            }
        }
        if m.peak_db > -60.0 {
            let px = x(frac(m.peak_db));
            painter.vline(px, rect.y_range(), Stroke::new(2.0, p.text));
        }
        ui.label(
            RichText::new(if m.db <= -89.0 { "-∞".to_string() } else { format!("{:.0}", m.db) })
                .size(SECONDARY)
                .color(p.secondary),
        );
    });
}

/// Mic choices; a saved mic that isn't connected stays listed as such.
fn mic_options(st: &State, s: &Settings) -> Vec<(Option<String>, String)> {
    let mut mics: Vec<(Option<String>, String)> = vec![(None, "Default microphone".into())];
    mics.extend(st.mics.iter().map(|d| (Some(d.id.clone()), d.name.clone())));
    if let Some(id) = &s.mic
        && !st.mics.iter().any(|d| &d.id == id)
    {
        let name = s.mic_name.clone().unwrap_or_else(|| id.clone());
        mics.push((Some(id.clone()), format!("{name} (not connected)")));
    }
    mics
}

/// Display name of the chosen mic (None for the system default).
fn mic_label(mics: &[(Option<String>, String)], id: &Option<String>) -> Option<String> {
    id.as_ref().and(mics.iter().find(|(v, _)| v == id).map(|(_, l)| l.trim_end_matches(" (not connected)").to_string()))
}

fn combo<T: Clone + PartialEq>(ui: &mut Ui, id: &str, value: &mut T, options: &[(T, String)], enabled: bool) -> bool {
    let current = options.iter().find(|(v, _)| v == value).map(|(_, l)| l.clone()).unwrap_or_default();
    let mut changed = false;
    ui.add_enabled_ui(enabled, |ui| {
        let resp = egui::ComboBox::from_id_salt(id).width(ui.available_width()).truncate().selected_text(current.clone()).show_ui(ui, |ui| {
            for (v, label) in options {
                if ui.selectable_label(v == value, label).clicked() && v != value {
                    *value = v.clone();
                    changed = true;
                }
            }
        });
        // Long names are cut to fit; show the whole one on hover.
        resp.response.on_hover_text(current);
    });
    changed
}

fn sources(ui: &mut Ui, st: &mut State) {
    let recording = st.recording.is_some() || st.finishing;
    let idle = !recording;
    let mut s = st.settings.clone();

    section(ui, "SCENE");
    // Two rows of two: four labels do not fit across the sidebar.
    for pair in Scene::ALL.chunks(2) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let w = (ui.available_width() - 4.0) / 2.0;
            for &scene in pair {
                // While recording, only scenes the running session supports.
                let ok = idle || st.can_switch_live(scene);
                let label = match scene {
                    Scene::Screen => "Screen",
                    Scene::ScreenCam => "Screen + Cam",
                    Scene::Presenter => "Presenter",
                    Scene::Cam => "Camera",
                };
                let sel = s.scene == scene;
                let b = egui::Button::selectable(sel, RichText::new(label).size(SECONDARY + 1.0)).min_size(vec2(w, 26.0));
                let resp = ui.add_enabled(ok, b);
                let resp = if ok { resp } else { resp.on_disabled_hover_text("Start the recording with a camera scene to switch live") };
                let resp = if scene == Scene::Presenter {
                    resp.on_hover_text("Your screen in a frame on a background, with the camera beside it")
                } else {
                    resp
                };
                if resp.clicked() && !sel {
                    s.scene = scene;
                    if let Some(r) = &st.recording {
                        r.set_scene(scene);
                    }
                }
            }
        });
    }

    section(ui, "SCREEN");
    let displays: Vec<(usize, String)> = st.displays.iter().enumerate().map(|(i, d)| (i + 1, d.clone())).collect();
    combo(ui, "display", &mut s.display, &displays, idle);
    ui.add_space(4.0);
    let mut targets: Vec<(Option<u32>, String)> = vec![(None, "Entire display".into())];
    targets.extend(st.windows.iter().map(|(id, l)| (Some(*id), l.clone())));
    ui.horizontal(|ui| {
        let w = ui.available_width() - 30.0;
        ui.allocate_ui(vec2(w, 24.0), |ui| {
            combo(ui, "window", &mut s.window, &targets, idle);
        });
        if ui.add_enabled(idle, egui::Button::new("↻")).on_hover_text("Refresh windows and devices").clicked() {
            st.refresh_devices = true;
        }
    });

    section(ui, "CAMERA");
    let mut cams: Vec<(Option<String>, String)> = vec![(None, "Default camera".into())];
    cams.extend(st.cameras.iter().map(|d| (Some(d.id.clone()), d.name.clone())));
    combo(ui, "camera", &mut s.camera, &cams, idle && s.scene.uses_cam());
    if !s.scene.uses_cam() {
        ui.label(RichText::new("Pick a camera scene to use the webcam.").size(SECONDARY).color(theme::pal(ui).secondary));
    }
    if s.scene.has_overlay() {
        ui.add_enabled(idle, egui::Checkbox::new(&mut s.save_camera, "Also save the camera on its own")).on_hover_text(
            "Writes NAME-camera next to the recording: the full camera picture with your mic, \
             in step with the recording frame for frame. Stack both in your editor to cut \
             between screen and talking head.",
        );
    }
    if s.scene.has_overlay() {
        section(ui, "CAMERA OVERLAY");
        let auto_h = auto_height(st, &s);
        overlay_controls(ui, &mut s, auto_h);
    }
    if s.scene == Scene::Presenter {
        section(ui, "BACKGROUND");
        background_controls(ui, &mut s);
    }

    section(ui, "AUDIO");
    ui.add_enabled(idle, egui::Checkbox::new(&mut s.system_audio, "System audio"));
    ui.add_enabled(idle, egui::Checkbox::new(&mut s.mic_enabled, "Microphone"));
    let mics = mic_options(st, &s);
    if combo(ui, "mic", &mut s.mic, &mics, idle && s.mic_enabled) {
        s.mic_name = mic_label(&mics, &s.mic);
    }
    ui.add_enabled(idle && s.mic_enabled, egui::Checkbox::new(&mut s.clean_voice, "Clean up voice")).on_hover_text(
        "Removes background noise (fans, hum, room tone), evens out your level and gently \
         compresses loud words, in the main audio track and streams. A raw copy of the mic \
         is always kept as its own track, so you can redo it in your editor.",
    );

    section(ui, "OUTPUT");
    let codec = s.codec.map_or("Auto codec".to_string(), |c| format!("{c:?}").replace("H264", "H.264").replace("Hevc", "HEVC").replace("Prores", "ProRes 422"));
    ui.label(
        RichText::new(format!("{} · {} fps · {} · .{}", s.resolution.to_uppercase(), s.fps, codec, s.container))
            .size(SECONDARY)
            .color(theme::pal(ui).secondary),
    );
    ui.label(RichText::new(tilde(&s.output_dir)).size(SECONDARY).color(theme::pal(ui).secondary));

    live_controls(ui, st, &mut s);

    if s != st.settings {
        st.settings = s;
        let _ = st.settings.save();
    }
}

fn bytes_text(b: usize) -> String {
    if b >= 1 << 30 { format!("{:.1} GB", b as f64 / (1u64 << 30) as f64) } else { format!("{:.0} MB", b as f64 / (1u64 << 20) as f64) }
}

pub const STREAM_DESTS: [(&str, &str); 3] = [("youtube", "YouTube"), ("twitch", "Twitch"), ("custom", "Custom (RTMP or WHIP URL)")];

/// Replay buffer and streaming: both use the shared live encode and run
/// with or without a recording.
fn live_controls(ui: &mut Ui, st: &mut State, s: &mut Settings) {
    let p = theme::pal(ui);
    section(ui, "REPLAY BUFFER");
    ui.horizontal(|ui| {
        ui.checkbox(&mut s.replay_enabled, "Keep the last");
        ui.spacing_mut().slider_width = (ui.available_width() - 60.0).max(60.0);
        ui.add(egui::Slider::new(&mut s.replay_secs, 15..=300).suffix(" s").step_by(5.0));
    });
    if let Some((held, bytes)) = st.replay_stats {
        ui.horizontal(|ui| {
            let tip = match st.replay_hotkey {
                Some(true) => "Save the buffer to an MP4 without stopping (⇧⌘S)",
                _ => "Save the buffer to an MP4 without stopping (⇧⌘S unavailable)",
            };
            if ui.add_enabled(held > 0.0, egui::Button::new("Save Replay")).on_hover_text(tip).clicked() {
                st.cmds.push(Cmd::SaveReplay);
            }
            ui.label(RichText::new(format!("{held:.0} s · {} in memory", bytes_text(bytes))).size(SECONDARY).color(p.secondary));
        });
    }

    section(ui, "STREAM");
    let live = st.stream_id.is_some();
    let dests: Vec<(String, String)> = STREAM_DESTS.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect();
    combo(ui, "stream-dest", &mut s.stream_dest, &dests, !live);
    if s.stream_dest == "custom" {
        ui.add_space(4.0);
        ui.add_enabled(
            !live,
            egui::TextEdit::singleline(&mut s.stream_custom_url)
                .hint_text("rtmp://… or https://…/whip")
                .desired_width(f32::INFINITY),
        );
    }
    // The key goes to the Keychain (per destination URL), never to settings.
    let url = s.stream_url();
    let has_key = if url == st.settings.stream_url() { st.has_stream_key() } else { false };
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if has_key && st.stream_key_input.is_empty() {
            ui.label(RichText::new("Stream key saved in Keychain").size(SECONDARY).color(p.secondary));
            if ui.add_enabled(!live, egui::Button::new("Remove")).clicked() {
                st.delete_stream_key();
            }
        } else {
            let w = ui.available_width() - 50.0;
            ui.add_sized(
                vec2(w, 22.0),
                egui::TextEdit::singleline(&mut st.stream_key_input).password(true).hint_text("Stream key"),
            );
            if ui.add_enabled(!st.stream_key_input.trim().is_empty(), egui::Button::new("Save")).clicked() {
                st.save_stream_key();
            }
        }
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let whip = url.starts_with("http");
        let ready = !url.is_empty() && (has_key || whip);
        let label = if live { "Stop Stream" } else { "Go Live" };
        let b = egui::Button::new(RichText::new(label).color(if live { Color32::WHITE } else { p.text })).fill(if live { RED } else { p.control });
        let resp = ui.add_enabled(live || ready, b);
        let resp = if ready || live { resp } else { resp.on_disabled_hover_text("Save a stream key first") };
        if resp.clicked() {
            st.cmds.push(Cmd::ToggleStream);
        }
        if let Some(status) = &st.stream_status {
            let text = if status.live {
                format!("{}  {:.1} Mb/s  {} dropped", crate::record::clock(status.duration), status.bitrate / 1e6, status.dropped_frames)
            } else {
                status.state.clone()
            };
            ui.label(RichText::new(text).size(SECONDARY).color(if status.live { p.text } else { p.secondary }));
        }
    });
    if let Some(e) = st.stream_status.as_ref().and_then(|s| (!s.live).then_some(s.error.as_ref()).flatten()) {
        ui.label(RichText::new(e).size(SECONDARY).color(RED));
    }
}

pub fn settings_window(ui: &mut Ui, st: &mut State) {
    let p = theme::pal(ui);
    let idle = st.recording.is_none() && !st.finishing;
    egui::Panel::top("settings-toolbar")
        .exact_size(TOOLBAR_H)
        .frame(egui::Frame::NONE.fill(p.window).inner_margin(Margin::symmetric(12, 0)))
        .show(ui, |ui| {
            toolbar(ui, |ui| {
                ui.label(RichText::new("Settings").font(theme::semibold(BODY)));
            });
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(p.window).inner_margin(Margin::symmetric(20, 12)))
        .show(ui, |ui| {
            if !idle {
                ui.label(RichText::new("Settings are locked while recording.").color(p.secondary));
            }
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.add_enabled_ui(idle, |ui| settings_form(ui, st));
                // The overlay stays editable while recording (applied live).
                section(ui, "CAMERA OVERLAY");
                let mut s = st.settings.clone();
                let auto_h = auto_height(st, &s);
                overlay_controls(ui, &mut s, auto_h);
                if s != st.settings {
                    st.settings = s;
                    st.apply_pip();
                    let _ = st.settings.save();
                }
            });
        });
}

fn settings_form(ui: &mut Ui, st: &mut State) {
    let mut s = st.settings.clone();
    let grid = |ui: &mut Ui, id: &str, add: &mut dyn FnMut(&mut Ui)| {
        egui::Grid::new(id).num_columns(2).spacing(vec2(12.0, 8.0)).min_col_width(110.0).show(ui, |ui| add(ui));
    };

    section(ui, "OUTPUT");
    grid(ui, "out", &mut |ui| {
        ui.label("Folder");
        ui.horizontal(|ui| {
            ui.label(RichText::new(tilde(&s.output_dir)).size(SECONDARY));
            if ui.button("Choose…").clicked()
                && let Some(d) = rfd::FileDialog::new().set_directory(&s.output_dir).pick_folder()
            {
                s.output_dir = d;
            }
        });
        ui.end_row();
        ui.label("Format");
        let opts = [("mov".to_string(), "QuickTime (.mov)".to_string()), ("mp4".to_string(), "MPEG-4 (.mp4)".to_string())];
        combo(ui, "container", &mut s.container, &opts, s.codec != Some(Codec::Prores));
        ui.end_row();
    });

    section(ui, "VIDEO");
    grid(ui, "video", &mut |ui| {
        ui.label("Resolution");
        let opts: Vec<(String, String)> = RESOLUTIONS.iter().map(|(l, v)| (v.to_string(), l.to_string())).collect();
        combo(ui, "res", &mut s.resolution, &opts, true);
        ui.end_row();
        ui.label("Frame rate");
        let opts: Vec<(u32, String)> = FPS.iter().map(|f| (*f, format!("{f} fps"))).collect();
        combo(ui, "fps", &mut s.fps, &opts, true);
        ui.end_row();
        ui.label("Codec");
        let opts = vec![
            (None, "Auto (H.264 ≤1080p, HEVC above)".to_string()),
            (Some(Codec::H264), "H.264".to_string()),
            (Some(Codec::Hevc), "HEVC (H.265)".to_string()),
            (Some(Codec::Prores), "ProRes 422 (.mov)".to_string()),
        ];
        combo(ui, "codec", &mut s.codec, &opts, true);
        ui.end_row();
        ui.label("Bitrate");
        ui.horizontal(|ui| {
            let mut auto = s.bitrate_mbps.is_none();
            if ui.add_enabled(s.codec != Some(Codec::Prores), egui::Checkbox::new(&mut auto, "Auto")).changed() {
                s.bitrate_mbps = if auto { None } else { Some(30.0) };
            }
            if let Some(b) = &mut s.bitrate_mbps {
                ui.add(egui::Slider::new(b, 2.0..=200.0).suffix(" Mb/s").logarithmic(true).integer());
            }
        });
        ui.end_row();
        ui.label("Cursor");
        ui.horizontal(|ui| {
            ui.checkbox(&mut s.cursor, "Show");
            ui.add_enabled(s.cursor, egui::Checkbox::new(&mut s.clicks, "Highlight clicks"));
        });
        ui.end_row();
        ui.label("Preview");
        ui.checkbox(&mut s.preview_low_power, "Low power (half resolution, 15 fps)");
        ui.end_row();
    });

    section(ui, "AUDIO");
    grid(ui, "audio", &mut |ui| {
        ui.label("System audio");
        ui.checkbox(&mut s.system_audio, "Record");
        ui.end_row();
        ui.label("Microphone");
        let mics = mic_options(st, &s);
        ui.horizontal(|ui| {
            ui.checkbox(&mut s.mic_enabled, "");
            if combo(ui, "mic2", &mut s.mic, &mics, s.mic_enabled) {
                s.mic_name = mic_label(&mics, &s.mic);
            }
        });
        ui.end_row();
        ui.label("Tracks");
        let opts = vec![
            (AudioLayout::Both, "Mix + separate system & mic".to_string()),
            (AudioLayout::Mix, "Mix only".to_string()),
            (AudioLayout::Stems, "Separate only".to_string()),
        ];
        combo(ui, "tracks", &mut s.audio_tracks, &opts, true);
        ui.end_row();
        ui.label("System gain");
        ui.add(egui::Slider::new(&mut s.system_gain_db, -24.0..=12.0).suffix(" dB").step_by(0.5));
        ui.end_row();
        ui.label("Mic gain");
        ui.add(egui::Slider::new(&mut s.mic_gain_db, -24.0..=24.0).suffix(" dB").step_by(0.5));
        ui.end_row();
    });

    section(ui, "LIVE ENCODE (REPLAY & STREAMING)");
    let streaming = st.stream_id.is_some();
    ui.add_enabled_ui(!streaming, |ui| {
        grid(ui, "live", &mut |ui| {
            ui.label("Resolution");
            let opts: Vec<(String, String)> =
                [("720p", "720p"), ("1080p", "1080p"), ("1440p", "1440p"), ("4k", "4K")].iter().map(|(v, l)| (v.to_string(), l.to_string())).collect();
            combo(ui, "live-res", &mut s.live_resolution, &opts, true);
            ui.end_row();
            ui.label("Codec");
            let opts: Vec<(String, String)> = [("auto", "Auto (HEVC for YouTube, else H.264)"), ("h264", "H.264"), ("hevc", "HEVC (falls back to H.264)")]
                .iter()
                .map(|(v, l)| (v.to_string(), l.to_string()))
                .collect();
            combo(ui, "live-codec", &mut s.stream_codec, &opts, true);
            ui.end_row();
            ui.label("Video bitrate");
            ui.horizontal(|ui| {
                ui.checkbox(&mut s.stream_bitrate_auto, "Platform preset");
                if !s.stream_bitrate_auto {
                    ui.add(egui::Slider::new(&mut s.stream_bitrate_kbps, 1000..=60_000).suffix(" kb/s").step_by(500.0).logarithmic(true));
                }
            });
            ui.end_row();
            ui.label("");
            let c = s.live_config();
            ui.label(
                RichText::new(format!(
                    "Up to {}x{} {} at {:.1} Mb/s (the frame keeps its shape, never above the recording size)",
                    c.width,
                    c.height,
                    if c.hevc { "HEVC" } else { "H.264" },
                    c.bitrate as f64 / 1e6
                ))
                .size(SECONDARY)
                .color(theme::pal(ui).secondary),
            );
            ui.end_row();
            ui.label("Rate control");
            ui.checkbox(&mut s.stream_cbr, "Constant bitrate (CBR)");
            ui.end_row();
            ui.label("Keyframe every");
            ui.add(egui::Slider::new(&mut s.keyframe_secs, 1.0..=4.0).suffix(" s").step_by(0.5));
            ui.end_row();
            ui.label("Audio bitrate");
            let opts: Vec<(u32, String)> = [96, 128, 160, 192, 256, 320].iter().map(|k| (*k, format!("{k} kb/s"))).collect();
            combo(ui, "live-audio", &mut s.stream_audio_kbps, &opts, true);
            ui.end_row();
        });
    });
    if streaming {
        ui.label(RichText::new("Stop the stream to change the live encode.").size(SECONDARY).color(theme::pal(ui).secondary));
    }

    section(ui, "CAMERA");
    grid(ui, "cam", &mut |ui| {
        ui.label("Camera");
        let mut cams: Vec<(Option<String>, String)> = vec![(None, "Default camera".into())];
        cams.extend(st.cameras.iter().map(|d| (Some(d.id.clone()), d.name.clone())));
        combo(ui, "cam2", &mut s.camera, &cams, true);
        ui.end_row();
    });
    ui.add_space(12.0);
    if ui.button("Restore Defaults").clicked() {
        s = crate::settings::Settings { output_dir: s.output_dir.clone(), ..Default::default() };
    }
    if s != st.settings {
        st.settings = s;
        let _ = st.settings.save();
    }
}
