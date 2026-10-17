//! Advanced view: dark, fluid dashboard (`design/Main.dc.html`). The layout
//! reproduces the CSS flex rules of the design; every fixed number below is
//! a box Chrome computes for the source at 1280 px wide.

use std::fmt::Write as _;

use app_core::{DenoiseLevel, ModelState, PresetId, UiState, presets::builtin};
use dsp::Module;

use crate::ballistics::{GrSmoother, Smoother};
use crate::input::{Action, Clicker, Cursor, Event, Key, ViewKind};
use crate::model_dialog::download_mark;
use crate::painter::{Align, Color, Family, LineCap, Painter, Point, Rect, Stroke, TextStyle};
use crate::spectrum::{Analyzer, BANDS, POINTS, band_center_hz, point_hz};
use crate::theme::advanced as th;
use crate::view::View;

pub const DEFAULT_WIDTH: f32 = 1280.0;
pub const DEFAULT_HEIGHT: f32 = 960.0;
const PAGE_PAD: f32 = 20.0;
const MAX_CONTENT_W: f32 = 1240.0;
const GAP: f32 = 16.0;

const CARD_MIN_W: f32 = 154.0;
const CARD_GAP: f32 = 10.0;
const CARD_H: f32 = 200.0;

/// Spectrum plot coordinate space.
const PLOT_W: f32 = 800.0;
const PLOT_H: f32 = 240.0;
const PLOT_AREA_H: f32 = 215.0;
const PLOT_DB_RANGE: f32 = 100.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Id {
    Ab,
    Mute,
    /// 0 = off, 1..=3 = `DenoiseLevel::ALL[i - 1]`.
    Noise(u8),
    Bypass,
    Compact,
    Preset(u8),
    Recalibrate,
    Input,
    Output,
    Module(u8),
    MonitorSwitch,
    MonitorLevel,
    MonitorDenoise,
}

/// Everything the layout depends on besides the size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct LayoutInputs {
    w: f32,
    h: f32,
    monitor_on: bool,
    monitor_denoise: bool,
    denoise_on: bool,
    /// Width of the voice profile text, px.
    profile_w: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct Layout {
    w: f32,
    content_x: f32,
    content_w: f32,
    header: Rect,
    /// Preset group (506 x 46).
    presets: Rect,
    noise_group: Rect,
    noise_label_w: f32,
    noise_buttons: [Rect; 4],
    noise_hint: Rect,
    profile: Rect,
    recal: Rect,
    spectrum: Rect,
    plot: Rect,
    loudness: Rect,
    chain_title_y: f32,
    cards: [Rect; 9],
    monitor: Rect,
    monitor_on: bool,
    footer_y: f32,
    total_h: f32,
    /// Height the content needs, independent of the size it was laid out
    /// in (so a window can shrink back when panels collapse).
    natural_h: f32,
}

pub struct AdvancedView {
    clicker: Clicker<Id>,
    layout: Layout,
    inputs: LayoutInputs,
    static_key: u64,
    analyzer_in: Analyzer,
    analyzer_out: Analyzer,
    eq_curve_db: [f32; BANDS],
    /// Spectrum curve in plot coordinates, reused every frame.
    curve_pts: Vec<Point>,
    gr: [GrSmoother; 9],
    momentary: Smoother,
    short_term: Smoother,
    true_peak: Smoother,
    in_peak: Smoother,
    lufs_valid: bool,
    /// Fixed analyzer curves (screenshots): skip the live analysis.
    demo_spectrum: bool,
    leveler_gain: Smoother,
    level_drag: bool,
    text: String,
    /// Right edge of each header control, recomputed at paint time.
    header_hits: [Rect; 5],
    /// The input device pill, placed while painting (its width follows the
    /// device name).
    input_rect: Rect,
    /// The monitor device button, placed while painting (empty when the
    /// monitor panel is collapsed).
    output_rect: Rect,
}

impl Default for AdvancedView {
    fn default() -> Self {
        Self::new()
    }
}

impl AdvancedView {
    pub fn new() -> Self {
        let mut v = Self {
            clicker: Clicker::new(),
            layout: Layout::default(),
            inputs: LayoutInputs::default(),
            static_key: 0,
            analyzer_in: Analyzer::new(dsp::SAMPLE_RATE as f32),
            analyzer_out: Analyzer::new(dsp::SAMPLE_RATE as f32),
            eq_curve_db: [0.0; BANDS],
            curve_pts: Vec::with_capacity(POINTS),
            gr: [GrSmoother::default(); 9],
            momentary: Smoother::new(-40.0, 0.2),
            short_term: Smoother::new(-40.0, 0.5),
            true_peak: Smoother::new(-40.0, 0.1),
            in_peak: Smoother::new(-60.0, 0.1),
            lufs_valid: false,
            demo_spectrum: false,
            leveler_gain: Smoother::new(0.0, 0.3),
            level_drag: false,
            text: String::with_capacity(128),
            header_hits: [Rect::default(); 5],
            input_rect: Rect::default(),
            output_rect: Rect::default(),
        };
        v.compute_layout(
            LayoutInputs {
                w: DEFAULT_WIDTH,
                h: DEFAULT_HEIGHT,
                ..LayoutInputs::default()
            },
            None,
        );
        v
    }

    /// Feeds raw samples from the engine's scope queues.
    pub fn feed(&mut self, input: &[f32], output: &[f32]) {
        self.analyzer_in.push(input);
        self.analyzer_out.push(output);
    }

    /// Fixed analyzer curves for screenshots: the shapes of the design.
    pub fn set_demo_spectrum(&mut self) {
        self.demo_spectrum = true;
        self.analyzer_in.set_demo(
            |f| {
                let l = (f / 500.0).log10();
                -34.0 - 14.0 * l * l - 2.0 * l
            },
            -82.0,
        );
        self.analyzer_out.set_demo(
            |f| {
                let l = (f / 600.0).log10();
                -32.0 - 22.0 * l * l + 1.0 * l
            },
            -96.0,
        );
        for (b, v) in self.eq_curve_db.iter_mut().enumerate() {
            let f = band_center_hz(b);
            let l = f.log10();
            *v = -3.7
                + 3.6 * (-((l - 2.0) * (l - 2.0)) / 0.06).exp()
                + 0.9 * (-((l - 3.4) * (l - 3.4)) / 0.5).exp()
                - 1.0 * (-((l - 2.5) * (l - 2.5)) / 0.03).exp();
        }
    }

    /// Lays out the page. `p` measures text; without it widths are
    /// estimated (only used before the first frame).
    fn compute_layout(&mut self, inputs: LayoutInputs, mut p: Option<&mut dyn Painter>) {
        let (w, h) = (inputs.w, inputs.h);
        let mut tw = |text: &str, style: &TextStyle| -> f32 {
            match p.as_deref_mut() {
                Some(p) => p.measure_text(text, style).width,
                None => text.chars().count() as f32 * style.size * 0.55,
            }
        };
        let content_w = (w - 2.0 * PAGE_PAD).clamp(320.0, MAX_CONTENT_W);
        let content_x = (w - content_w) / 2.0;
        let mut y = PAGE_PAD;
        let header = Rect::new(content_x, y, content_w, 64.0);
        y += 64.0 + GAP;

        // Row 2: [presets][noise removal][voice profile] ... [recalibrate],
        // CSS flex-wrap with 12 px gaps.
        let row_y = y;
        let noise_label_w = tw("Noise removal", &sans(12.0, 400, th::MUTED)) + 16.0;
        let mut btn_w = [72.0f32; 4];
        for (i, label) in NOISE_LABELS.iter().enumerate() {
            btn_w[i] = (tw(label, &sans(13.0, 500, th::TEXT)) + 24.0).max(72.0);
        }
        let hint = if inputs.denoise_on { HINT_ON } else { HINT_OFF };
        let hint_w = tw(hint, &mono(12.0, 400, th::MUTED)) + 16.0;
        let noise_w =
            4.0 + noise_label_w + 4.0 + btn_w.iter().sum::<f32>() + 12.0 + 4.0 + hint_w + 4.0;
        let profile_w = inputs.profile_w;
        let items = [(506.0f32, 46.0f32), (noise_w, 46.0), (profile_w, 17.0)];
        let mut rects = [Rect::default(); 3];
        let mut line_x = content_x;
        let mut line_y = row_y;
        let mut line_h = 0.0f32;
        let mut left_w = 0.0f32;
        for (i, (iw, ih)) in items.iter().enumerate() {
            if line_x > content_x && line_x + iw > content_x + content_w + 0.01 {
                line_y += line_h + 12.0;
                line_x = content_x;
                line_h = 0.0;
            }
            rects[i] = Rect::new(line_x, line_y, *iw, *ih);
            line_h = line_h.max(*ih);
            line_x += iw + 12.0;
            left_w = left_w.max(line_x - 12.0 - content_x);
        }
        // Center items vertically within their line.
        for r in rects.iter_mut() {
            let lh = if r.y == row_y && left_w >= 506.0 {
                46.0
            } else {
                r.h.max(17.0)
            };
            let lh = if (r.y - row_y).abs() < 0.01 {
                46.0f32.max(lh)
            } else {
                line_h.max(lh)
            };
            r.y += (lh - r.h) / 2.0;
        }
        let left_h = line_y + line_h - row_y;
        let recal_w =
            tw("Recalibrate (15 s)", &sans(13.0, 400, th::TEXT)) + 14.0 + 16.0 + 8.0 + 14.0;
        let (recal, row_h) = if left_w + 12.0 + recal_w <= content_w {
            (
                Rect::new(content_x + content_w - recal_w, row_y + 3.0, recal_w, 40.0),
                left_h.max(46.0),
            )
        } else {
            (
                Rect::new(content_x, row_y + left_h + 12.0, recal_w, 40.0),
                left_h + 12.0 + 40.0,
            )
        };
        let presets = rects[0];
        let noise_group = rects[1];
        let mut noise_buttons = [Rect::default(); 4];
        let mut bx = noise_group.x + 4.0 + noise_label_w + 4.0;
        for i in 0..4 {
            noise_buttons[i] = Rect::new(bx, noise_group.y + 5.0, btn_w[i], 36.0);
            bx += btn_w[i] + 4.0;
        }
        let noise_hint = Rect::new(bx, noise_group.y, hint_w, 46.0);
        let profile = rects[2];
        y += row_h + GAP;

        // Spectrum (flex 999 1 560) + loudness (flex 1 1 260), 16 gap.
        let (spectrum, loudness, plot);
        if content_w >= 904.0 {
            let free = content_w - 904.0;
            let sw = 594.0 + free * 0.999;
            let lw = content_w - sw - GAP;
            let plot_w = sw - 34.0;
            let plot_h = plot_w * PLOT_H / PLOT_W;
            let sh = 62.0 + plot_h;
            spectrum = Rect::new(content_x, y, sw, sh);
            plot = Rect::new(content_x + 17.0, y + 45.0, plot_w, plot_h);
            loudness = Rect::new(content_x + sw + GAP, y, lw, sh.max(324.0));
            y += sh.max(324.0) + GAP;
        } else {
            let plot_w = content_w - 34.0;
            let plot_h = plot_w * PLOT_H / PLOT_W;
            let sh = 62.0 + plot_h;
            spectrum = Rect::new(content_x, y, content_w, sh);
            plot = Rect::new(content_x + 17.0, y + 45.0, plot_w, plot_h);
            y += sh + GAP;
            loudness = Rect::new(content_x, y, content_w, 324.0);
            y += 324.0 + GAP;
        }

        let chain_title_y = y;
        y += 18.0 + 10.0;
        let per_row =
            (((content_w + CARD_GAP) / (CARD_MIN_W + CARD_GAP)).floor() as usize).clamp(1, 9);
        let mut cards = [Rect::default(); 9];
        let mut i = 0;
        while i < 9 {
            let n = (9 - i).min(per_row);
            let cw = (content_w - CARD_GAP * (n as f32 - 1.0)) / n as f32;
            for k in 0..n {
                cards[i + k] = Rect::new(content_x + (cw + CARD_GAP) * k as f32, y, cw, CARD_H);
            }
            y += CARD_H + CARD_GAP;
            i += n;
        }
        y += GAP - CARD_GAP;

        let (mon_on, mon_denoise) = (inputs.monitor_on, inputs.monitor_denoise);
        let mon_h = if mon_on {
            201.0 + if mon_denoise { 29.0 } else { 0.0 }
        } else {
            100.0
        };
        let monitor = Rect::new(content_x, y, content_w, mon_h);
        y += mon_h + GAP;
        let footer_y = y;
        let natural_h = y + 15.0 + PAGE_PAD;
        let total_h = natural_h.max(h);

        self.inputs = inputs;
        self.layout = Layout {
            w,
            content_x,
            content_w,
            header,
            presets,
            noise_group,
            noise_label_w,
            noise_buttons,
            noise_hint,
            profile,
            recal,
            spectrum,
            plot,
            loudness,
            chain_title_y,
            cards,
            monitor,
            monitor_on: mon_on,
            footer_y,
            total_h,
            natural_h,
        };
        self.static_key = self.static_key.wrapping_add(1);
    }

    /// Height of the whole page at the current width (for scrolling hosts).
    pub fn content_height(&self) -> f32 {
        self.layout.total_h
    }

    /// Height the content needs at the current width and state.
    pub fn natural_height(&self) -> f32 {
        self.layout.natural_h
    }

    fn preset_rect(&self, i: usize) -> Rect {
        let g = self.layout.presets;
        Rect::new(g.x + 5.0 + 100.0 * i as f32, g.y + 5.0, 96.0, 36.0)
    }

    fn power_rect(card: Rect) -> Rect {
        Rect::new(card.right() - 13.0 - 44.0, card.y + 13.0, 44.0, 44.0)
    }

    fn monitor_switch_rect(&self) -> Rect {
        let m = self.layout.monitor;
        Rect::new(m.right() - 17.0 - 56.0, m.y + 18.5, 56.0, 32.0)
    }

    fn monitor_columns(&self) -> [Rect; 3] {
        let m = self.layout.monitor;
        let inner_w = m.w - 34.0;
        let free = (inner_w - 732.0).max(0.0);
        let w0 = 220.0 + free / 3.0;
        let w2 = 260.0 + free / 3.0;
        let x0 = m.x + 17.0;
        let y = m.y + 66.0;
        [
            Rect::new(x0, y, w0, 67.0),
            Rect::new(x0 + w0 + GAP, y, w0, 67.0),
            Rect::new(x0 + 2.0 * (w0 + GAP), y, w2, 67.0),
        ]
    }

    fn level_slider_rect(&self) -> Rect {
        let c = self.monitor_columns()[1];
        Rect::new(c.x, c.y + 23.0, c.w, 40.0)
    }

    fn denoise_button_rect(&self) -> Rect {
        let c = self.monitor_columns()[2];
        Rect::new(c.x, c.y + 23.0, c.w, 40.0)
    }

    fn hit(&self, p: Point) -> Option<Id> {
        if self.input_rect.contains(p) {
            return Some(Id::Input);
        }
        if self.output_rect.contains(p) {
            return Some(Id::Output);
        }
        let ids = [Id::Ab, Id::Bypass, Id::Compact, Id::Mute];
        for (i, r) in self.header_hits.iter().enumerate().skip(1) {
            if r.contains(p) {
                return Some(ids[i - 1]);
            }
        }
        for i in 0..5 {
            if self.preset_rect(i).contains(p) {
                return Some(Id::Preset(i as u8));
            }
        }
        if self.layout.recal.contains(p) {
            return Some(Id::Recalibrate);
        }
        for (i, r) in self.layout.noise_buttons.iter().enumerate() {
            if r.contains(p) {
                return Some(Id::Noise(i as u8));
            }
        }
        for (i, c) in self.layout.cards.iter().enumerate() {
            if Self::power_rect(*c).contains(p) {
                return Some(Id::Module(i as u8));
            }
        }
        if self.monitor_switch_rect().contains(p) {
            return Some(Id::MonitorSwitch);
        }
        if self.layout.monitor_on {
            if self.level_slider_rect().contains(p) {
                return Some(Id::MonitorLevel);
            }
            if self.denoise_button_rect().contains(p) {
                return Some(Id::MonitorDenoise);
            }
        }
        None
    }

    fn activate(&self, id: Id, state: &UiState, out: &mut Vec<Action>) {
        match id {
            Id::Ab => out.push(Action::SetAbOriginal(!state.ab_original)),
            Id::Mute => out.push(Action::SetMuted(!state.muted)),
            Id::Bypass => out.push(Action::SetOn(!state.on)),
            Id::Compact => out.push(Action::SwitchView(ViewKind::Standard)),
            Id::Preset(i) => out.push(Action::SetPreset(PresetId::ALL[i as usize])),
            Id::Recalibrate => out.push(Action::Recalibrate),
            Id::Input => out.push(Action::ChooseInput(self.input_rect)),
            Id::Output => out.push(Action::ChooseOutput(self.output_rect)),
            Id::Module(i) => out.push(Action::SetModuleEnabled(
                Module::ALL[i as usize],
                !state.module_enabled[i as usize],
            )),
            Id::Noise(0) => out.push(Action::SetNeuralDenoise(None)),
            Id::Noise(i) => out.push(Action::SetNeuralDenoise(Some(
                DenoiseLevel::ALL[i as usize - 1],
            ))),
            Id::MonitorSwitch => {
                let mut m = state.monitor;
                m.enabled = !m.enabled;
                out.push(Action::SetMonitor(m));
            }
            Id::MonitorDenoise => {
                let mut m = state.monitor;
                m.with_denoise = !m.with_denoise;
                out.push(Action::SetMonitor(m));
            }
            Id::MonitorLevel => {}
        }
    }

    fn level_from_pointer(&self, p: Point) -> f32 {
        let r = self.level_slider_rect();
        ((p.x - r.x - 8.0) / (r.w - 16.0)).clamp(0.0, 1.0)
    }

    fn plot_x(f: f32) -> f32 {
        186.0 + (f / 100.0).log10() * 267.0
    }

    fn plot_y(db: f32) -> f32 {
        (-db / PLOT_DB_RANGE * PLOT_AREA_H).clamp(-20.0, PLOT_AREA_H)
    }

    fn eq_y(gain_db: f32) -> f32 {
        40.0 - gain_db * 6.0
    }

    /// Fills `pts` with an analyzer curve in plot coordinates.
    fn spectrum_points(pts: &mut Vec<Point>, levels: &[f32]) {
        pts.clear();
        pts.extend(
            levels
                .iter()
                .enumerate()
                .map(|(i, &db)| Point::new(Self::plot_x(point_hz(i)), Self::plot_y(db))),
        );
    }

    fn polyline(p: &mut dyn Painter, pts: &[Point], stroke: Stroke) {
        p.begin_path();
        for (i, pt) in pts.iter().enumerate() {
            if i == 0 {
                p.move_to(pt.x, pt.y);
            } else {
                p.line_to(pt.x, pt.y);
            }
        }
        p.stroke(stroke);
    }

    fn curve(p: &mut dyn Painter, ys: impl Fn(usize) -> f32, stroke: Stroke) {
        p.begin_path();
        for b in 0..BANDS {
            let x = Self::plot_x(band_center_hz(b));
            let y = ys(b);
            if b == 0 {
                p.move_to(x, y);
            } else {
                p.line_to(x, y);
            }
        }
        p.stroke(stroke);
    }
}

/// Height of a noise floor label (12 px mono) plus a gap, plot units.
const NOISE_LABEL_H: f32 = 14.0;

/// Baselines of the "noise in" and "noise out" labels for floor lines at
/// `in_y` and `out_y`: above each line, except that when the lines are
/// closer than a label, the lower line's label goes under its line.
fn noise_label_baselines(in_y: f32, out_y: f32) -> (f32, f32) {
    let above = |y: f32| y - 4.0;
    let below = |y: f32| y + NOISE_LABEL_H - 1.0;
    if (in_y - out_y).abs() >= NOISE_LABEL_H {
        (above(in_y), above(out_y))
    } else if out_y >= in_y {
        (above(in_y), below(out_y))
    } else {
        (below(in_y), above(out_y))
    }
}

const NOISE_LABELS: [&str; 4] = ["Off", "Light", "Medium", "Strong"];
const HINT_ON: &str = "+20 ms";
const HINT_OFF: &str = "lowest latency";
const HINT_MODEL: &str = "35 MB model";
/// Space between the voice profile and the preset's use.
const USE_FOR_GAP: f32 = 24.0;

/// "<profile> with <microphone>", the voice profile line.
fn write_profile(text: &mut String, state: &UiState) {
    let profile = if state.voice_profile.is_empty() {
        "Default"
    } else {
        &state.voice_profile
    };
    let dev = if state.input_device.is_empty() {
        "Default microphone"
    } else {
        &state.input_device
    };
    let _ = write!(text, "{profile} with {dev}");
}

/// `text` cut with an ellipsis to fit `max` wide in `style`.
fn elide(p: &mut dyn Painter, text: &str, style: &TextStyle, max: f32) -> String {
    if p.measure_text(text, style).width <= max {
        return text.to_string();
    }
    let mut cut = text.len();
    while cut > 0 {
        cut = text[..cut].char_indices().last().map_or(0, |(i, _)| i);
        let s = format!("{}…", text[..cut].trim_end());
        if p.measure_text(&s, style).width <= max {
            return s;
        }
    }
    "…".to_string()
}

fn mix(c: Color, bg: Color, k: f32) -> Color {
    Color::rgba(
        bg.r + (c.r - bg.r) * k,
        bg.g + (c.g - bg.g) * k,
        bg.b + (c.b - bg.b) * k,
        1.0,
    )
}

fn sans(size: f32, weight: u16, color: Color) -> TextStyle {
    TextStyle::new(Family::PlexSans, weight, size, color)
}

fn mono(size: f32, weight: u16, color: Color) -> TextStyle {
    TextStyle::new(Family::PlexMono, weight, size, color)
}

/// Bordered control box (buttons, pills), CSS `border: 1px` inside `r`.
fn control(p: &mut dyn Painter, r: Rect, radius: f32, bg: Color, border: Color) {
    p.fill_rounded_rect(r, radius, bg);
    p.border_rounded_rect(r, radius, 1.0, border);
}

fn bar(p: &mut dyn Painter, r: Rect, fraction: f32, color: Color) {
    p.fill_rounded_rect(r, r.h / 2.0, th::CONTROL);
    let w = r.w * fraction.clamp(0.0, 1.0);
    if w > 0.0 {
        p.fill_rounded_rect(Rect::new(r.x, r.y, w, r.h), r.h / 2.0, color);
    }
}

fn m_badge(p: &mut dyn Painter, x: f32, y: f32) {
    p.fill_rounded_rect(Rect::new(x, y, 18.0, 18.0), 4.0, th::PHONES_BG);
    p.text_in_rect(
        "M",
        Rect::new(x, y, 18.0, 18.0),
        &mono(11.0, 400, th::PHONES),
        Align::Center,
    );
}

fn power_icon(p: &mut dyn Painter, cx: f32, cy: f32, color: Color) {
    // 16 x 16 icon centered at (cx, cy): stem (8,2)-(8,8) and an arc of
    // radius 5 around (8,8) from (4.5,4.5) the long way to (11.5,4.5).
    let s = Stroke::new(color, 1.8).cap(LineCap::Round);
    let ox = cx - 8.0;
    let oy = cy - 8.0;
    p.line(ox + 8.0, oy + 2.0, ox + 8.0, oy + 8.0, s);
    p.begin_path();
    let a0 = (-3.5f32).atan2(-3.5);
    let a1 = (-3.5f32).atan2(3.5);
    p.arc(ox + 8.0, oy + 8.0, 5.0, a0, a1 + std::f32::consts::TAU);
    p.stroke(s);
}

fn module_macro(state: &UiState, i: usize, text: &mut String) -> (&'static str, f32) {
    let m = &state.chain.modules;
    let target = builtin(state.preset).target_lufs;
    text.clear();
    match Module::ALL[i] {
        Module::Input => {
            let _ = write!(text, "{:+.0} dB", m.input_trim_db);
            ("Gain", ((m.input_trim_db + 12.0) / 36.0).clamp(0.0, 1.0))
        }
        Module::Denoise => {
            text.push_str(state.neural_denoise.map_or("Off", DenoiseLevel::label));
            match state.neural_denoise {
                None => ("Expander only", 0.0),
                Some(DenoiseLevel::Light) => ("Neural + expander", 0.33),
                Some(DenoiseLevel::Medium) => ("Neural + expander", 0.66),
                Some(DenoiseLevel::Strong) => ("Neural + expander", 1.0),
            }
        }
        Module::DePlosive => {
            let _ = write!(text, "{:.0} dB", m.deplosive_threshold_db);
            (
                "Threshold",
                ((m.deplosive_threshold_db + 60.0) / 60.0).clamp(0.0, 1.0),
            )
        }
        Module::DeEsser => {
            let _ = write!(text, "{:.1} kHz", m.deesser_freq_hz / 1000.0);
            (
                "Frequency (auto)",
                ((m.deesser_freq_hz - 3000.0) / 8000.0).clamp(0.0, 1.0),
            )
        }
        Module::DynamicEq => {
            let active = m
                .eq_dynamic_threshold_db
                .iter()
                .filter(|t| **t < 0.0)
                .count();
            let _ = write!(text, "{active} / 4");
            ("Active bands", active as f32 / 4.0)
        }
        Module::Compressor => {
            let _ = write!(text, "{:.1} : 1", m.comp_ratio);
            ("Amount", ((m.comp_ratio - 1.0) / 6.0).clamp(0.0, 1.0))
        }
        Module::Saturation => {
            let _ = write!(text, "{:.0}%", m.saturation_drive * 100.0);
            ("Warmth", m.saturation_drive.clamp(0.0, 1.0))
        }
        Module::Leveler => {
            let _ = write!(text, "{:.0} LUFS", target);
            ("Target", ((target + 24.0) / 12.0).clamp(0.0, 1.0))
        }
        Module::Limiter => {
            let _ = write!(text, "{:.1} dBTP", m.limiter_ceiling_dbtp);
            (
                "Ceiling",
                ((m.limiter_ceiling_dbtp + 6.0) / 6.0).clamp(0.0, 1.0),
            )
        }
    }
}

/// Second row of a module card: label, formatted value, bar fraction.
fn module_meter(i: usize, gr_db: f32, text: &mut String) -> (&'static str, f32) {
    text.clear();
    match Module::ALL[i] {
        Module::Denoise => {
            let _ = write!(text, "-{:.0} dB", gr_db);
            ("Noise", gr_db / 30.0)
        }
        Module::Saturation => {
            let _ = write!(text, "{:.1} dB", gr_db);
            ("Drive", gr_db / 6.0)
        }
        Module::Leveler => {
            let _ = write!(text, "{:+.1} dB", gr_db);
            ("Gain", gr_db.abs() / 12.0)
        }
        _ => {
            if gr_db > 0.05 {
                let _ = write!(text, "-{:.1} dB", gr_db);
            } else {
                text.push_str("0.0 dB");
            }
            ("GR", gr_db / 15.0)
        }
    }
}

fn module_latency_ms(i: usize, denoise_on: bool) -> f32 {
    match Module::ALL[i] {
        Module::Denoise if denoise_on => 20.0,
        Module::Denoise => 0.0,
        Module::Saturation => 0.6,
        Module::Limiter => 1.5,
        _ => 0.0,
    }
}

impl View for AdvancedView {
    fn preferred_size(&self) -> (f32, f32) {
        (DEFAULT_WIDTH, DEFAULT_HEIGHT)
    }

    fn layout(&mut self, w: f32, h: f32, state: &UiState, p: &mut dyn Painter) {
        let profile_w = {
            let muted13 = sans(13.0, 400, th::MUTED);
            self.text.clear();
            write_profile(&mut self.text, state);
            p.measure_text("Voice profile: ", &muted13).width
                + p.measure_text(&self.text, &sans(13.0, 400, th::TEXT)).width
                + USE_FOR_GAP
                + p.measure_text(builtin(state.preset).use_for, &muted13)
                    .width
        };
        let inputs = LayoutInputs {
            w,
            h,
            monitor_on: state.monitor.enabled,
            monitor_denoise: state.monitor.with_denoise,
            denoise_on: state.neural_denoise.is_some(),
            profile_w: profile_w.round(),
        };
        if inputs != self.inputs {
            self.compute_layout(inputs, Some(p));
        }
    }

    fn static_key(&self) -> u64 {
        self.static_key
    }

    fn tick(&mut self, state: &UiState, dt: f32) {
        let m = &state.meters;
        for (g, v) in self.gr.iter_mut().zip(m.gain_reduction_db) {
            g.update(v.abs(), dt);
        }
        // No loudness below the -70 LUFS gate (silence, muted): show --.-.
        self.lufs_valid = m.lufs_short_term.is_finite();
        if self.lufs_valid {
            self.momentary.update(m.lufs_momentary, dt);
            self.short_term.update(m.lufs_short_term, dt);
        }
        self.true_peak.update(m.true_peak_db, dt);
        self.in_peak.update(m.in_peak_db, dt);
        self.leveler_gain
            .update(-m.gain_reduction_db[Module::Leveler.index()], dt);
        if self.demo_spectrum {
            return;
        }
        self.analyzer_in.update(dt);
        self.analyzer_out.update(dt);
        // EQ action: static shaping gains at the four band centers.
        let centers = [250.0f32, 1000.0, 4000.0, 10_000.0];
        let gains = state.chain.modules.eq_static_db;
        for (b, v) in self.eq_curve_db.iter_mut().enumerate() {
            let f = band_center_hz(b).log10();
            let mut g = 0.0;
            for (c, gain) in centers.iter().zip(gains) {
                let d = f - c.log10();
                g += gain * (-d * d / 0.08).exp();
            }
            *v = g;
        }
    }

    fn paint_static(&mut self, p: &mut dyn Painter) {
        let l = self.layout;
        p.fill_rect(Rect::new(0.0, 0.0, l.w, l.total_h), th::BG);

        // Header shell and static left group.
        control(p, l.header, 12.0, th::PANEL, th::BORDER);
        let hx = l.header.x + 17.0;
        let hy = l.header.y + 13.0;
        let logo = Stroke::new(th::ACCENT, 2.0).cap(LineCap::Round);
        let (lx, ly) = (hx, hy + 5.0);
        p.line(lx + 4.0, ly + 14.0, lx + 6.0, ly + 14.0, logo);
        p.line(lx + 9.0, ly + 9.0, lx + 9.0, ly + 19.0, logo);
        p.line(lx + 14.0, ly + 5.0, lx + 14.0, ly + 23.0, logo);
        p.line(lx + 19.0, ly + 9.0, lx + 19.0, ly + 19.0, logo);
        p.line(lx + 24.0, ly + 14.0, lx + 22.0, ly + 14.0, logo);
        let title = sans(17.0, 600, th::TEXT).spacing(0.2);
        p.text_in_rect(
            "Voice Strip",
            Rect::new(hx + 38.0, hy, 200.0, 38.0),
            &title,
            Align::Left,
        );

        // Preset and noise removal group shells, recalibrate button.
        control(p, l.presets, 10.0, th::PANEL, th::BORDER);
        control(p, l.noise_group, 10.0, th::PANEL, th::BORDER);
        p.text_in_rect(
            "Noise removal",
            Rect::new(
                l.noise_group.x + 12.0,
                l.noise_group.y,
                l.noise_label_w,
                46.0,
            ),
            &sans(12.0, 400, th::MUTED),
            Align::Left,
        );
        let recal_label = "Recalibrate (15 s)";
        let rr = l.recal;
        let rw = rr.w;
        p.border_rounded_rect(rr, 8.0, 1.0, th::BORDER);
        let (cx, cy) = (rr.x + 15.0 + 8.0, rr.y + 20.0);
        p.begin_path();
        p.circle(cx, cy, 6.0);
        p.stroke(Stroke::new(th::TEXT, 1.6));
        let hand = Stroke::new(th::TEXT, 1.6).cap(LineCap::Round);
        p.line(cx, cy - 3.5, cx, cy, hand);
        p.line(cx, cy, cx + 2.5, cy + 1.5, hand);
        // The label is dynamic (calibration progress): see paint_dynamic.
        let _ = (recal_label, rw);

        // Spectrum panel.
        control(p, l.spectrum, 12.0, th::PANEL, th::BORDER);
        let sx = l.spectrum.x + 17.0;
        let sy = l.spectrum.y + 17.0;
        p.text_in_rect(
            "Spectrum",
            Rect::new(sx, sy, 100.0, 18.0),
            &sans(14.0, 600, th::TEXT),
            Align::Left,
        );
        let legend = sans(12.0, 400, th::MUTED);
        let items: [(&str, Color, bool); 4] = [
            ("Input", th::DIM, false),
            ("Output", th::ACCENT, false),
            ("EQ action", th::PHONES, false),
            ("Voice band", th::VOICE_BAND, true),
        ];
        let mut widths = [0.0f32; 4];
        let mut total = 0.0;
        for (i, (label, _, _)) in items.iter().enumerate() {
            widths[i] = 14.0 + 6.0 + p.measure_text(label, &legend).width;
            total += widths[i];
        }
        total += 14.0 * 3.0;
        let mut x = l.spectrum.right() - 17.0 - total;
        for (i, (label, color, band)) in items.iter().enumerate() {
            if *band {
                p.fill_rect(Rect::new(x + 1.0, sy + 4.0, 14.0, 8.0), *color);
                p.begin_path();
                p.rect(Rect::new(x + 0.5, sy + 3.5, 15.0, 9.0));
                p.stroke(Stroke::new(th::VOICE_BAND_BORDER, 1.0));
            } else {
                p.fill_rect(Rect::new(x, sy + 8.0, 14.0, 2.0), *color);
            }
            p.text_in_rect(
                label,
                Rect::new(x + 20.0, sy + 1.5, widths[i], 15.0),
                &legend,
                Align::Left,
            );
            x += widths[i] + 14.0;
        }
        // Plot background: voice band, grid, axis labels.
        p.save();
        p.translate(l.plot.x, l.plot.y);
        let k = l.plot.w / PLOT_W;
        p.scale(k, k);
        p.fill_rect(
            Rect::new(160.0, 0.0, 581.0, PLOT_AREA_H),
            th::ACCENT.with_alpha(0.07),
        );
        let grid = Stroke::new(th::GRID, 1.0);
        for y in [50.0, 100.0, 150.0, 200.0] {
            p.line(0.0, y, PLOT_W, y, grid);
        }
        for x in [186.0, 453.0, 720.0] {
            p.line(x, 0.0, x, PLOT_AREA_H, grid);
        }
        let axis = mono(12.0, 400, th::MUTED);
        for (label, x) in [
            ("20", 4.0),
            ("100", 178.0),
            ("1k", 444.0),
            ("10k", 708.0),
            ("Hz", 770.0),
        ] {
            p.text_baseline(label, x, 234.0, &axis, Align::Left);
        }
        p.restore();

        // Loudness panel shell and static labels.
        control(p, l.loudness, 12.0, th::PANEL, th::BORDER);
        let lx = l.loudness.x + 17.0;
        let ly = l.loudness.y + 17.0;
        p.text_in_rect(
            "Loudness",
            Rect::new(lx, ly, 100.0, 18.0),
            &sans(14.0, 600, th::TEXT),
            Align::Left,
        );
        let rows = ["Momentary", "Short-term", "True peak", "Input (peak)"];
        for (i, label) in rows.iter().enumerate() {
            let ry = ly + 134.0 + 37.0 * i as f32;
            p.fill_text(label, lx, ry, &sans(12.0, 400, th::MUTED), Align::Left);
        }

        // Chain title and note.
        p.text_in_rect(
            "Chain",
            Rect::new(l.content_x, l.chain_title_y, 100.0, 18.0),
            &sans(14.0, 600, th::TEXT),
            Align::Left,
        );
        let note = sans(12.0, 400, th::MUTED);
        let a = "Fixed order. ";
        let b = " = included in headphone monitoring";
        let aw = p.measure_text(a, &note).width;
        let bw = p.measure_text(b, &note).width;
        let right = l.content_x + l.content_w;
        let x0 = right - aw - 18.0 - bw;
        let row = Rect::new(x0, l.chain_title_y, aw, 18.0);
        p.text_in_rect(a, row, &note, Align::Left);
        m_badge(p, x0 + aw, l.chain_title_y);
        p.text_in_rect(
            b,
            Rect::new(x0 + aw + 18.0, l.chain_title_y, bw, 18.0),
            &note,
            Align::Left,
        );

        // Monitoring panel shell, icon, titles.
        control(p, l.monitor, 12.0, th::PANEL, th::BORDER);
        let mx = l.monitor.x + 17.0;
        let my = l.monitor.y + 17.0;
        let hp = Stroke::new(th::PHONES, 1.8).cap(LineCap::Round);
        let (ix, iy) = (mx, my + 6.5);
        p.begin_path();
        p.move_to(ix + 3.0, iy + 14.0);
        p.line_to(ix + 3.0, iy + 11.0);
        p.arc(
            ix + 11.0,
            iy + 11.0,
            8.0,
            std::f32::consts::PI,
            std::f32::consts::TAU,
        );
        p.line_to(ix + 19.0, iy + 14.0);
        p.stroke(hp);
        p.begin_path();
        p.rounded_rect(Rect::new(ix + 2.5, iy + 13.0, 4.0, 6.0), 1.5);
        p.stroke(hp);
        p.begin_path();
        p.rounded_rect(Rect::new(ix + 15.5, iy + 13.0, 4.0, 6.0), 1.5);
        p.stroke(hp);
        p.fill_text(
            "Headphone monitoring",
            mx + 32.0,
            my,
            &sans(14.0, 600, th::TEXT),
            Align::Left,
        );
        p.fill_text(
            "Optional. Off by default.",
            mx + 32.0,
            my + 20.0,
            &sans(12.0, 400, th::MUTED),
            Align::Left,
        );
        if l.monitor_on {
            let cols = self.monitor_columns();
            let small = sans(12.0, 400, th::MUTED);
            p.fill_text("Device", cols[0].x, cols[0].y, &small, Align::Left);
            p.fill_text("Level", cols[1].x, cols[1].y, &small, Align::Left);
            p.fill_text(
                "Neural denoise in monitoring",
                cols[2].x,
                cols[2].y,
                &small,
                Align::Left,
            );
        } else {
            p.fill_text(
                "Off: all processing goes to the virtual microphone only, no extra load. Turn it on to hear yourself in headphones while you speak.",
                mx,
                my + 49.0,
                &sans(13.0, 400, th::MUTED),
                Align::Left,
            );
        }

        // Footer.
        let foot = mono(12.0, 400, th::MUTED);
        p.fill_text(
            "48 kHz · float32 · buffer 128",
            l.content_x,
            l.footer_y,
            &foot,
            Align::Left,
        );
    }

    fn paint_dynamic(&mut self, p: &mut dyn Painter, state: &UiState) {
        let l = self.layout;
        let on = state.on;

        // Recalibrate button label: idle text or calibration progress.
        let recal_text = state
            .calibration
            .as_ref()
            .map_or_else(|| "Recalibrate (15 s)".to_string(), |c| c.label());
        let recal_color = if state.calibration.as_ref().is_some_and(|c| c.error) {
            th::ACCENT
        } else {
            th::TEXT
        };
        p.text_in_rect(
            &recal_text,
            Rect::new(l.recal.x + 15.0 + 24.0, l.recal.y, l.recal.w, 40.0),
            &sans(13.0, 400, recal_color),
            Align::Left,
        );

        // Header: devices (left) and controls (right).
        let hy = l.header.y + 13.0;
        let s13 = sans(13.0, 400, th::TEXT);
        let muted13 = sans(13.0, 400, th::MUTED);
        let mut x = l.header.x
            + 17.0
            + 38.0
            + p.measure_text("Voice Strip", &sans(17.0, 600, th::TEXT).spacing(0.2))
                .width
            + 16.0;

        // Right group, laid out from the right edge.
        let m13 = mono(13.0, 400, th::TEXT);
        let mut right = l.header.right() - 17.0;
        let cw = p.measure_text("Compact view", &s13).width + 24.0;
        let compact = Rect::new(right - cw, hy, cw, 38.0);
        p.border_rounded_rect(compact, 8.0, 1.0, th::BORDER);
        let link_color = if self.clicker.hover == Some(Id::Compact) {
            th::ACCENT_HOVER
        } else {
            th::ACCENT
        };
        p.text_in_rect(
            "Compact view",
            Rect::new(compact.x + 12.0, hy, cw, 38.0),
            &sans(13.0, 400, link_color),
            Align::Left,
        );
        right -= cw + 8.0;
        let bypass = !on;
        let byw = p.measure_text("Bypass", &sans(13.0, 500, th::TEXT)).width + 28.0;
        let byr = Rect::new(right - byw, hy + 1.0, byw, 36.0);
        control(
            p,
            byr,
            8.0,
            if bypass { th::ACCENT } else { th::CONTROL },
            if bypass { th::ACCENT } else { th::BORDER },
        );
        p.text_in_rect(
            "Bypass",
            byr,
            &sans(13.0, 500, if bypass { th::ACCENT_INK } else { th::TEXT }),
            Align::Center,
        );
        right -= byw + 8.0;
        let muted = state.muted;
        let mute_label = if muted { "Muted" } else { "Mute" };
        let mw = p.measure_text("Muted", &sans(13.0, 600, th::TEXT)).width + 46.0;
        let mr = Rect::new(right - mw, hy + 1.0, mw, 36.0);
        control(
            p,
            mr,
            8.0,
            if muted { th::MUTE_ON } else { th::CONTROL },
            if muted { th::MUTE_ON } else { th::BORDER },
        );
        // Microphone glyph, struck through when muted.
        let gx = mr.x + 16.0;
        let gy = mr.y + 18.0;
        let ink = if muted { th::TEXT } else { th::TEXT_SOFT };
        p.fill_rounded_rect(Rect::new(gx - 3.0, gy - 8.0, 6.0, 10.0), 3.0, ink);
        let wire = Stroke::new(ink, 1.4).cap(LineCap::Round);
        p.line(gx - 5.0, gy - 1.0, gx - 5.0, gy + 1.0, wire);
        p.line(gx + 5.0, gy - 1.0, gx + 5.0, gy + 1.0, wire);
        p.line(gx - 5.0, gy + 1.0, gx, gy + 5.0, wire);
        p.line(gx + 5.0, gy + 1.0, gx, gy + 5.0, wire);
        p.line(gx, gy + 5.0, gx, gy + 8.0, wire);
        if muted {
            p.line(
                gx - 7.0,
                gy + 8.0,
                gx + 7.0,
                gy - 9.0,
                Stroke::new(th::TEXT, 1.8).cap(LineCap::Round),
            );
        }
        p.text_in_rect(
            mute_label,
            Rect::new(mr.x + 28.0, mr.y, mw - 28.0, mr.h),
            &sans(13.0, if muted { 600 } else { 500 }, th::TEXT),
            Align::Center,
        );
        right -= mw + 8.0;
        let ab_label = if state.ab_original {
            "B orig."
        } else {
            "A proc."
        };
        let abw = (p.measure_text(ab_label, &m13).width + 24.0).max(64.0);
        let abr = Rect::new(right - abw, hy + 1.0, abw, 36.0);
        control(p, abr, 8.0, th::CONTROL, th::BORDER);
        p.text_in_rect(ab_label, abr, &m13, Align::Center);
        right -= abw + 8.0;
        self.header_hits = [Rect::default(), abr, byr, compact, mr];
        if state.monitor.enabled {
            self.text.clear();
            let _ = write!(self.text, "Phones {:.1} ms", state.monitor_latency_ms);
            let w = p.measure_text(&self.text, &m13).width + 10.0 + 8.0 + 6.0 + 10.0;
            let r = Rect::new(right - w, hy + 2.0, w, 34.0);
            p.border_rounded_rect(r, 17.0, 1.0, th::BORDER);
            p.fill_circle(r.x + 15.0, r.y + 17.0, 4.0, th::PHONES);
            p.text_in_rect(
                &self.text,
                Rect::new(r.x + 24.0, r.y, w, 34.0),
                &m13,
                Align::Left,
            );
            right -= w + 8.0;
        }
        self.text.clear();
        let _ = write!(self.text, "Mic {:.1} ms", state.mic_latency_ms);
        let w = p.measure_text(&self.text, &m13).width + 10.0 + 8.0 + 6.0 + 10.0;
        let r = Rect::new(right - w, hy + 2.0, w, 34.0);
        p.border_rounded_rect(r, 17.0, 1.0, th::BORDER);
        p.fill_circle(r.x + 15.0, r.y + 17.0, 4.0, th::ACCENT);
        p.text_in_rect(
            &self.text,
            Rect::new(r.x + 24.0, r.y, w, 34.0),
            &m13,
            Align::Left,
        );

        // Devices, after the right group so they can stop before it.
        let limit = r.x - 16.0;
        p.text_in_rect("Input", Rect::new(x, hy, 60.0, 38.0), &muted13, Align::Left);
        x += p.measure_text("Input", &muted13).width + 8.0;
        let dev = if state.input_device.is_empty() {
            "Default microphone"
        } else {
            &state.input_device
        };
        let kw = p.measure_text("48 kHz", &muted13).width;
        // A long microphone name is shortened so the devices end before
        // the right group.
        let vdev = if state.virtual_device.is_empty() {
            "Virtual microphone"
        } else {
            &state.virtual_device
        };
        let after = 8.0
            + 18.0
            + 8.0
            + p.measure_text("Output", &muted13).width
            + 8.0
            + p.measure_text(vdev, &s13).width
            + 24.0;
        let room = limit - x - (12.0 + 8.0 + kw + 12.0) - after;
        let dev = elide(p, dev, &s13, room);
        let dev = dev.as_str();
        let dw = p.measure_text(dev, &s13).width;
        let bw = 12.0 + dw + 8.0 + kw + 12.0;
        self.input_rect = Rect::new(x, hy + 1.0, bw, 36.0);
        control(
            p,
            Rect::new(x, hy + 1.0, bw, 36.0),
            8.0,
            th::CONTROL,
            th::BORDER,
        );
        p.text_in_rect(
            dev,
            Rect::new(x + 12.0, hy + 1.0, dw, 36.0),
            &s13,
            Align::Left,
        );
        p.text_in_rect(
            "48 kHz",
            Rect::new(x + 12.0 + dw + 8.0, hy + 1.0, kw, 36.0),
            &muted13,
            Align::Left,
        );
        x += bw + 8.0;
        let arrow = Stroke::new(th::MUTED, 1.6).cap(LineCap::Round);
        let ay = hy + 19.0;
        p.line(x + 3.0, ay, x + 14.0, ay, arrow);
        p.line(x + 10.0, ay - 4.0, x + 14.0, ay, arrow);
        p.line(x + 14.0, ay, x + 10.0, ay + 4.0, arrow);
        x += 18.0 + 8.0;
        p.text_in_rect(
            "Output",
            Rect::new(x, hy, 60.0, 38.0),
            &muted13,
            Align::Left,
        );
        x += p.measure_text("Output", &muted13).width + 8.0;
        let vw = p.measure_text(vdev, &s13).width + 24.0;
        control(p, Rect::new(x, hy, vw, 38.0), 8.0, th::CONTROL, th::BORDER);
        p.text_in_rect(vdev, Rect::new(x + 12.0, hy, vw, 38.0), &s13, Align::Left);

        // Presets and voice profile.
        for (i, id) in PresetId::ALL.iter().enumerate() {
            let r = self.preset_rect(i);
            let active = *id == state.preset;
            if active {
                p.fill_rounded_rect(r, 7.0, th::ACCENT);
            }
            p.text_in_rect(
                builtin(*id).label,
                r,
                &sans(
                    13.0,
                    500,
                    if active {
                        th::ACCENT_INK
                    } else {
                        th::TEXT_SOFT
                    },
                ),
                Align::Center,
            );
        }
        // Noise removal group: Off / Light / Medium / Strong plus hint.
        let denoise_on = state.neural_denoise.is_some();
        for (i, r) in l.noise_buttons.iter().enumerate() {
            let selected = match i {
                0 => !denoise_on,
                _ => state.neural_denoise == Some(DenoiseLevel::ALL[i - 1]),
            };
            let (bg, fg) = match (selected, i) {
                (true, 0) => (Some(th::BORDER), th::TEXT),
                (true, _) => (Some(th::ACCENT), th::ACCENT_INK),
                (false, _) => (None, th::TEXT_SOFT),
            };
            if let Some(bg) = bg {
                p.fill_rounded_rect(*r, 7.0, bg);
            }
            p.text_in_rect(NOISE_LABELS[i], *r, &sans(13.0, 500, fg), Align::Center);
        }
        let hint = Rect::new(
            l.noise_hint.x + 8.0,
            l.noise_hint.y,
            l.noise_hint.w,
            l.noise_hint.h,
        );
        if state.model == ModelState::Ready {
            p.text_in_rect(
                if denoise_on { HINT_ON } else { HINT_OFF },
                hint,
                &mono(12.0, 400, if denoise_on { th::ACCENT } else { th::DIM }),
                Align::Left,
            );
        } else {
            // The levels need the model: say what it costs.
            download_mark(p, hint.x, hint.y + (hint.h - 9.0) / 2.0, th::DIM);
            p.text_in_rect(
                HINT_MODEL,
                Rect::new(hint.x + 13.0, hint.y, hint.w - 13.0, hint.h),
                &mono(12.0, 400, th::DIM),
                Align::Left,
            );
        }
        let pr = l.profile;
        p.text_in_rect("Voice profile: ", pr, &muted13, Align::Left);
        let pw = p.measure_text("Voice profile: ", &muted13).width;
        self.text.clear();
        write_profile(&mut self.text, state);
        p.text_in_rect(
            &self.text,
            Rect::new(pr.x + pw, pr.y, pr.w, pr.h),
            &s13,
            Align::Left,
        );
        // What the selected preset is for, after the profile.
        let used = pw + p.measure_text(&self.text, &s13).width + USE_FOR_GAP;
        p.text_in_rect(
            builtin(state.preset).use_for,
            Rect::new(pr.x + used, pr.y, pr.w - used, pr.h),
            &muted13,
            Align::Left,
        );

        // Spectrum curves.
        p.save();
        p.translate(l.plot.x, l.plot.y);
        let k = l.plot.w / PLOT_W;
        p.scale(k, k);
        p.clip_rect(Rect::new(0.0, -2.0, PLOT_W, PLOT_AREA_H + 4.0));
        let noise_in = Self::plot_y(self.analyzer_in.noise_floor_db());
        let noise_out = Self::plot_y(self.analyzer_out.noise_floor_db());
        p.line(
            0.0,
            noise_in,
            PLOT_W,
            noise_in,
            Stroke::new(th::DIM, 1.0).dash(4.0, 4.0),
        );
        p.line(
            0.0,
            noise_out,
            PLOT_W,
            noise_out,
            Stroke::new(th::ACCENT.with_alpha(0.7), 1.0).dash(4.0, 4.0),
        );
        // Input: a dim fill and line. Output on top: a gradient fill that
        // fades towards the floor, the held peaks, and a bright line.
        Self::spectrum_points(&mut self.curve_pts, self.analyzer_in.points());
        p.fill_under(
            &self.curve_pts,
            PLOT_AREA_H,
            th::DIM.with_alpha(0.16),
            th::DIM.with_alpha(0.0),
        );
        Self::polyline(p, &self.curve_pts, Stroke::new(th::DIM, 1.5));
        Self::spectrum_points(&mut self.curve_pts, self.analyzer_out.peaks());
        Self::polyline(
            p,
            &self.curve_pts,
            Stroke::new(th::ACCENT.with_alpha(0.35), 1.0),
        );
        Self::spectrum_points(&mut self.curve_pts, self.analyzer_out.points());
        p.fill_under(
            &self.curve_pts,
            PLOT_AREA_H,
            th::ACCENT.with_alpha(0.32),
            th::ACCENT.with_alpha(0.02),
        );
        Self::polyline(p, &self.curve_pts, Stroke::new(th::ACCENT, 2.0));
        let eq = self.eq_curve_db;
        Self::curve(
            p,
            |b| Self::eq_y(eq[b]),
            Stroke::new(th::PHONES, 2.0).dash(6.0, 4.0),
        );
        let axis = mono(12.0, 400, th::MUTED);
        let (in_base, out_base) = noise_label_baselines(noise_in, noise_out);
        p.text_baseline("noise in", 6.0, in_base, &axis, Align::Left);
        p.text_baseline(
            "noise out",
            6.0,
            out_base,
            &axis.color(th::ACCENT),
            Align::Left,
        );
        p.restore();

        // Loudness values.
        let lx = l.loudness.x + 17.0;
        let ly = l.loudness.y + 17.0;
        let big = mono(44.0, 500, th::ACCENT);
        self.text.clear();
        if self.lufs_valid {
            let _ = write!(self.text, "{:.1}", self.short_term.value());
        } else {
            self.text.push_str("--.-");
        }
        let big_size = p.measure_text(&self.text, &big);
        p.fill_text(&self.text, lx, ly + 32.0, &big, Align::Left);
        let unit = sans(13.0, 400, th::MUTED);
        let unit_size = p.measure_text("LUFS short-term", &unit);
        p.fill_text(
            "LUFS short-term",
            lx + big_size.width + 8.0,
            ly + 32.0 + big_size.ascent - unit_size.ascent,
            &unit,
            Align::Left,
        );
        p.fill_text("Target ", lx, ly + 103.0, &muted13, Align::Left);
        let tw = p.measure_text("Target ", &muted13).width;
        self.text.clear();
        let _ = write!(self.text, "{:.0} LUFS", builtin(state.preset).target_lufs);
        p.fill_text(&self.text, lx + tw, ly + 103.0, &m13, Align::Left);
        let bar_w = l.loudness.w - 34.0;
        let m11 = mono(12.0, 400, th::TEXT);
        let rows: [(f32, f32, Color, &str); 4] = [
            (
                self.momentary.value(),
                (self.momentary.value() + 40.0) / 34.0,
                th::ACCENT,
                "",
            ),
            (
                self.short_term.value(),
                (self.short_term.value() + 40.0) / 34.0,
                th::ACCENT,
                "",
            ),
            (
                self.true_peak.value(),
                (self.true_peak.value() + 10.0) / 10.0,
                th::TEXT,
                " dBTP",
            ),
            (
                self.in_peak.value(),
                (self.in_peak.value() + 24.0) / 24.0,
                th::DIM,
                " dBFS",
            ),
        ];
        for (i, (value, frac, color, unit)) in rows.iter().enumerate() {
            let ry = ly + 134.0 + 37.0 * i as f32;
            self.text.clear();
            if i < 2 && !self.lufs_valid {
                self.text.push_str("--.-");
            } else if value.is_finite() {
                let _ = write!(self.text, "{value:.1}{unit}");
            } else {
                let _ = write!(self.text, "-inf{unit}");
            }
            p.fill_text(&self.text, lx + bar_w, ry, &m11, Align::Right);
            bar(
                p,
                Rect::new(lx, ry + 19.0, bar_w, 8.0),
                if value.is_finite() { *frac } else { 0.0 },
                *color,
            );
        }

        // Module cards.
        for (i, card) in l.cards.iter().enumerate() {
            let enabled = state.module_enabled[i] && on;
            let dim = if enabled { 1.0 } else { 0.45 };
            let c = |col: Color| mix(col, th::BG, dim);
            p.fill_rounded_rect(*card, 10.0, c(th::PANEL));
            p.border_rounded_rect(
                *card,
                10.0,
                1.0,
                c(if enabled {
                    th::BORDER
                } else {
                    th::PANEL_OFF_BORDER
                }),
            );
            let ix = card.x + 13.0;
            let iw = card.w - 26.0;
            self.text.clear();
            let _ = write!(self.text, "{:02}", i + 1);
            p.fill_text(
                &self.text,
                ix,
                card.y + 18.0,
                &mono(11.0, 400, c(th::MUTED)),
                Align::Left,
            );
            p.fill_text(
                Module::ALL[i].name(),
                ix,
                card.y + 34.0,
                &sans(14.0, 600, c(th::TEXT)),
                Align::Left,
            );
            let pr = Self::power_rect(*card);
            let pc = pr.center();
            p.fill_circle(
                pc.x,
                pc.y,
                22.0,
                c(if enabled { th::ACCENT_BG } else { th::CONTROL }),
            );
            p.begin_path();
            p.circle(pc.x, pc.y, 21.5);
            p.stroke(Stroke::new(
                c(if enabled { th::ACCENT } else { th::BORDER }),
                1.0,
            ));
            power_icon(p, pc.x, pc.y, c(if enabled { th::ACCENT } else { th::DIM }));

            let (macro_label, amount) = module_macro(state, i, &mut self.text);
            p.fill_text(
                &self.text,
                ix,
                card.y + 69.0,
                &mono(18.0, 500, c(th::TEXT)),
                Align::Left,
            );
            p.fill_text(
                macro_label,
                ix,
                card.y + 96.0,
                &sans(12.0, 400, c(th::MUTED)),
                Align::Left,
            );
            bar(
                p,
                Rect::new(ix, card.y + 115.0, iw, 6.0),
                amount,
                c(th::TEXT),
            );

            let meter_value = if Module::ALL[i] == Module::Leveler {
                self.leveler_gain.value()
            } else {
                self.gr[i].value()
            };
            let (gr_label, gr_frac) = module_meter(i, meter_value, &mut self.text);
            p.fill_text(
                gr_label,
                ix,
                card.y + 133.0,
                &sans(11.0, 400, c(th::MUTED)),
                Align::Left,
            );
            p.fill_text(
                &self.text,
                ix + iw,
                card.y + 133.0,
                &mono(11.0, 400, c(th::MUTED)),
                Align::Right,
            );
            bar(
                p,
                Rect::new(ix, card.y + 151.0, iw, 6.0),
                gr_frac,
                c(th::ACCENT),
            );

            self.text.clear();
            let lat = module_latency_ms(i, state.neural_denoise.is_some());
            if lat.fract() == 0.0 {
                let _ = write!(self.text, "{lat:.0} ms");
            } else {
                let _ = write!(self.text, "{lat:.1} ms");
            }
            p.text_in_rect(
                &self.text,
                Rect::new(ix, card.y + 169.0, iw, 18.0),
                &mono(11.0, 400, c(th::MUTED)),
                Align::Left,
            );
            if Module::ALL[i].in_monitor_path() {
                p.fill_rounded_rect(
                    Rect::new(ix + iw - 18.0, card.y + 169.0, 18.0, 18.0),
                    4.0,
                    c(th::PHONES_BG),
                );
                p.text_in_rect(
                    "M",
                    Rect::new(ix + iw - 18.0, card.y + 169.0, 18.0, 18.0),
                    &mono(11.0, 400, c(th::PHONES)),
                    Align::Center,
                );
            }
        }

        // Monitoring controls.
        let mon = state.monitor;
        let sw = self.monitor_switch_rect();
        control(
            p,
            sw,
            16.0,
            if mon.enabled {
                th::PHONES_SWITCH_BG
            } else {
                th::CONTROL
            },
            if mon.enabled { th::PHONES } else { th::BORDER },
        );
        let kx = if mon.enabled { sw.x + 27.0 } else { sw.x + 3.0 };
        p.fill_circle(
            kx + 12.0,
            sw.y + 15.0,
            12.0,
            if mon.enabled { th::PHONES } else { th::DIM },
        );
        self.output_rect = Rect::default();
        if l.monitor_on {
            let cols = self.monitor_columns();
            let dev_r = Rect::new(cols[0].x, cols[0].y + 23.0, cols[0].w, 40.0);
            self.output_rect = dev_r;
            control(p, dev_r, 8.0, th::CONTROL, th::BORDER);
            let out_name = if state.output_device.is_empty() {
                "Headphones"
            } else {
                state.output_device.as_str()
            };
            p.text_in_rect(
                out_name,
                Rect::new(dev_r.x + 12.0, dev_r.y, dev_r.w, 40.0),
                &s13,
                Align::Left,
            );
            p.text_in_rect(
                "buffer 64",
                Rect::new(dev_r.x, dev_r.y, dev_r.w - 12.0, 40.0),
                &mono(13.0, 400, th::MUTED),
                Align::Right,
            );
            self.text.clear();
            let _ = write!(self.text, "{:.0}%", mon.level * 100.0);
            p.fill_text(
                &self.text,
                cols[1].right(),
                cols[1].y,
                &mono(12.0, 400, th::TEXT),
                Align::Right,
            );
            let sl = self.level_slider_rect();
            let ty = sl.y + 20.0;
            p.fill_rounded_rect(
                Rect::new(sl.x + 2.0, ty - 2.0, sl.w - 4.0, 4.0),
                2.0,
                th::BORDER,
            );
            let fx = sl.x + 8.0 + (sl.w - 16.0) * mon.level.clamp(0.0, 1.0);
            p.fill_rounded_rect(
                Rect::new(sl.x + 2.0, ty - 2.0, fx - sl.x - 2.0, 4.0),
                2.0,
                th::PHONES,
            );
            p.fill_circle(fx, ty, 8.0, th::PHONES);
            let dn = self.denoise_button_rect();
            control(
                p,
                dn,
                8.0,
                if mon.with_denoise {
                    th::ACCENT_BG
                } else {
                    th::CONTROL
                },
                if mon.with_denoise {
                    th::ACCENT
                } else {
                    th::BORDER
                },
            );
            p.text_in_rect(
                if mon.with_denoise {
                    "Included (+20 ms)"
                } else {
                    "Excluded (low latency)"
                },
                Rect::new(dn.x + 12.0, dn.y, dn.w, 40.0),
                &sans(
                    13.0,
                    400,
                    if mon.with_denoise {
                        th::ACCENT
                    } else {
                        th::TEXT
                    },
                ),
                Align::Left,
            );
            let path_r = Rect::new(
                l.monitor.x + 17.0,
                l.monitor.y + 130.0,
                l.monitor.w - 34.0,
                37.0,
            );
            control(p, path_r, 8.0, th::PATH_BG, th::PATH_BORDER);
            let lead = sans(12.0, 600, th::PHONES);
            p.text_in_rect(
                "Headphone path",
                Rect::new(path_r.x + 13.0, path_r.y, 200.0, 37.0),
                &lead,
                Align::Left,
            );
            let lw = p.measure_text("Headphone path", &lead).width;
            self.text.clear();
            for (n, m) in Module::ALL
                .iter()
                .filter(|m| m.in_monitor_path() || (mon.with_denoise && **m == Module::Denoise))
                .enumerate()
            {
                if n > 0 {
                    self.text.push_str(", ");
                }
                self.text.push_str(m.name());
            }
            p.text_in_rect(
                &self.text,
                Rect::new(path_r.x + 13.0 + lw + 8.0, path_r.y, 800.0, 37.0),
                &sans(12.0, 400, th::TEXT_SOFT),
                Align::Left,
            );
            self.text.clear();
            let _ = write!(self.text, "{:.1} ms", state.monitor_latency_ms);
            p.text_in_rect(
                &self.text,
                Rect::new(path_r.x, path_r.y, path_r.w - 13.0, 37.0),
                &mono(12.0, 400, th::TEXT),
                Align::Right,
            );
            if mon.with_denoise {
                p.fill_text(
                    "Above 10 ms your own voice in headphones sounds phasey. Recommended only in very noisy rooms.",
                    l.monitor.x + 17.0,
                    l.monitor.y + 198.0,
                    &sans(12.0, 400, th::ACCENT),
                    Align::Left,
                );
            }
        }

        // Footer notice (left of CPU), when there is one.
        if let Some(notice) = &state.notice {
            p.fill_text(
                notice,
                l.content_x + l.content_w - 120.0,
                l.footer_y,
                &sans(12.0, 400, th::ACCENT),
                Align::Right,
            );
        }

        // Footer CPU.
        self.text.clear();
        let _ = write!(self.text, "CPU {:.1}%", state.cpu_percent);
        p.fill_text(
            &self.text,
            l.content_x + l.content_w,
            l.footer_y,
            &mono(12.0, 400, th::MUTED),
            Align::Right,
        );
    }

    fn handle(&mut self, ev: Event, state: &UiState, out: &mut Vec<Action>) {
        match ev {
            Event::PointerMove(pt) => {
                if self.level_drag {
                    let mut m = state.monitor;
                    m.level = self.level_from_pointer(pt);
                    out.push(Action::SetMonitor(m));
                } else {
                    self.clicker.moved(self.hit(pt));
                }
            }
            Event::PointerDown(pt) => {
                let hit = self.hit(pt);
                self.clicker.down(hit);
                if hit == Some(Id::MonitorLevel) {
                    self.level_drag = true;
                    let mut m = state.monitor;
                    m.level = self.level_from_pointer(pt);
                    out.push(Action::SetMonitor(m));
                }
            }
            Event::PointerUp(pt) => {
                self.level_drag = false;
                if let Some(id) = self.clicker.up(self.hit(pt)) {
                    self.activate(id, state, out);
                }
            }
            Event::PointerLeave => {
                self.level_drag = false;
                self.clicker.leave();
            }
            Event::Scroll(..) => {}
            Event::Key(k) => match k {
                Key::Char(c) => match c {
                    '1'..='5' => {
                        out.push(Action::SetPreset(PresetId::ALL[c as usize - '1' as usize]))
                    }
                    'b' | 'B' => out.push(Action::SetOn(!state.on)),
                    'a' | 'A' => out.push(Action::SetAbOriginal(!state.ab_original)),
                    'm' | 'M' => out.push(Action::SetMuted(!state.muted)),
                    'h' | 'H' => {
                        let mut m = state.monitor;
                        m.enabled = !m.enabled;
                        out.push(Action::SetMonitor(m));
                    }
                    _ => {}
                },
                Key::Escape => out.push(Action::SwitchView(ViewKind::Standard)),
                _ => {}
            },
        }
    }

    fn tooltip(&self, p: Point, state: &UiState) -> Option<String> {
        use crate::help;
        let id = self.hit(p)?;
        Some(match id {
            Id::Ab => help::ab(state),
            Id::Mute => help::mute(state),
            Id::Noise(i) => help::noise(
                if i == 0 {
                    None
                } else {
                    Some(DenoiseLevel::ALL[i as usize - 1])
                },
                state,
            ),
            Id::Bypass => help::bypass(state),
            Id::Compact => help::compact(),
            Id::Preset(i) => help::preset(PresetId::ALL[i as usize], state),
            Id::Recalibrate => help::calibrate(),
            Id::Input => help::device_input(state),
            Id::Output => help::device_output(state),
            Id::Module(i) => help::module(Module::ALL[i as usize], state),
            Id::MonitorSwitch => help::monitor(state),
            Id::MonitorLevel => help::monitor_level(),
            Id::MonitorDenoise => help::monitor_denoise(),
        })
    }

    fn cursor(&self) -> Cursor {
        match self.clicker.hover {
            Some(_) => Cursor::Pointer,
            None => Cursor::Default,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_labels_never_overlap() {
        for (a, b) in [
            (100.0, 100.0),
            (100.0, 105.0),
            (105.0, 100.0),
            (100.0, 113.9),
        ] {
            let (ia, ib) = noise_label_baselines(a, b);
            // Each label spans its baseline minus 12 px of text height.
            let overlap = (ia - ib).abs() < 12.0;
            assert!(!overlap, "floors at {a} and {b}: labels at {ia} and {ib}");
        }
        // Far apart, both labels sit above their lines.
        assert_eq!(noise_label_baselines(50.0, 150.0), (46.0, 146.0));
    }

    #[test]
    fn cards_wrap_like_the_design_at_1280() {
        let mut v = AdvancedView::new();
        v.compute_layout(
            LayoutInputs {
                w: 1280.0,
                h: 960.0,
                denoise_on: true,
                profile_w: 290.0,
                ..LayoutInputs::default()
            },
            None,
        );
        let c = v.layout.cards;
        // Seven cards of 168.6 on the first line, two of 615 on the second.
        assert!((c[0].w - 168.6).abs() < 0.1, "w = {}", c[0].w);
        assert!((c[6].x - 1091.4).abs() < 0.1);
        assert!((c[7].y - (c[0].y + 210.0)).abs() < 0.01, "y = {}", c[7].y);
        assert!((c[7].w - 615.0).abs() < 0.1);
        assert!((v.layout.spectrum.w - 929.7).abs() < 0.1);
        assert!((v.layout.loudness.x - 965.7).abs() < 0.1);
        assert!((c[0].y - (v.layout.chain_title_y + 28.0)).abs() < 0.01);
        assert!((v.layout.monitor.y - (c[7].bottom() + 16.0)).abs() < 0.01);
        // Row 2 at 1280: presets, noise group and the recalibrate button on
        // the first line; the voice profile wraps below it.
        assert_eq!(v.layout.presets.y, v.layout.noise_group.y);
        assert!(v.layout.recal.y < v.layout.presets.bottom());
        assert!(v.layout.profile.y >= v.layout.presets.bottom());
    }

    #[test]
    fn plot_axis_matches_design() {
        assert!((AdvancedView::plot_x(100.0) - 186.0).abs() < 0.01);
        assert!((AdvancedView::plot_x(1000.0) - 453.0).abs() < 0.01);
        assert!((AdvancedView::plot_x(10_000.0) - 720.0).abs() < 0.01);
        assert!(AdvancedView::plot_x(20.0).abs() < 1.0);
    }

    #[test]
    fn module_toggle_action() {
        let mut v = AdvancedView::new();
        let s = crate::demo::demo_state();
        let c = Self_power_center(&v, 2);
        let mut out = Vec::new();
        v.handle(Event::PointerDown(c), &s, &mut out);
        v.handle(Event::PointerUp(c), &s, &mut out);
        assert_eq!(
            out,
            vec![Action::SetModuleEnabled(Module::DePlosive, false)]
        );
    }

    #[allow(non_snake_case)]
    fn Self_power_center(v: &AdvancedView, i: usize) -> Point {
        AdvancedView::power_rect(v.layout.cards[i]).center()
    }
}
