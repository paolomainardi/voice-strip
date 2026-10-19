//! Standard view: 420 x 600 vintage hardware faceplate
//! (`design/Standard.dc.html`). Every coordinate below is the CSS layout
//! box Chrome computes for the design source, in logical pixels.

use std::fmt::Write as _;

use app_core::{DenoiseLevel, ModelState, PresetId, UiState, presets::builtin};

use crate::ballistics::{GrSmoother, PeakHold, Smoother, VuBallistics};
use crate::input::{Action, Clicker, Cursor, Event, Key, ViewKind};
use crate::model_dialog::download_mark;
use crate::painter::{Align, Color, Family, LineCap, Painter, Point, Rect, Stroke, TextStyle};
use crate::theme::standard as th;
use crate::view::View;
use crate::{knob, vu};

pub const WIDTH: f32 = 420.0;
/// Extra height below the presets for the preset explanation, beyond the
/// design source.
const DY: f32 = 24.0;
pub const HEIGHT: f32 = 648.0 + DY;

/// 0 VU on the meters corresponds to this RMS level in dBFS.
pub const VU_REFERENCE_DBFS: f32 = -18.0;
/// Levels that drive the input and output VU needles, dB relative to the
/// VU reference, before ballistics: the average-reading rectifier, as on a
/// real VU. The window and remote clients (the bar widget) share them.
pub fn vu_inputs_db(m: &engine::MeterFrame) -> (f32, f32) {
    (
        m.in_avg_db - VU_REFERENCE_DBFS,
        m.out_avg_db - VU_REFERENCE_DBFS,
    )
}

/// Peak lamp threshold, input sample peak in dBFS.
pub const PEAK_LAMP_DBFS: f32 = -0.5;

const FACEPLATE: Rect = Rect::new(10.0, 10.0, 400.0, 628.0 + DY);
const SCREWS: [(f32, f32); 4] = [
    (17.0, 17.0),
    (397.0, 17.0),
    (17.0, 625.0 + DY),
    (397.0, 625.0 + DY),
];

const TITLE: Rect = Rect::new(30.0, 28.0, 126.2, 30.0);
const SUBTITLE: Rect = Rect::new(30.0, 58.0, 126.2, 16.0);
/// A newer release replaces the subtitle with a lamp and UPDATE x.y.z.
const UPDATE_HIT: Rect = Rect::new(26.0, 54.0, 150.0, 22.0);
/// Right edge of a footer notice: it wraps on two lines before CALIBRATE.
const NOTICE_RIGHT: f32 = 194.0;
const ON_BUTTON_Y: f32 = 29.0;
const ON_BUTTON_H: f32 = 44.0;
const CONTENT_RIGHT: f32 = 390.0;
/// MUTE button size; it sits 8 px left of the ON/BYPASS button.
const MUTE_W: f32 = 92.0;

const VU_IN: Rect = Rect::new(30.0, 88.0, 174.0, 113.1);
const VU_OUT: Rect = Rect::new(216.0, 88.0, 174.0, 113.1);
const VU_LABEL_Y: f32 = 205.1;
const VU_LABEL_H: f32 = 17.0;
const PEAK_LAMP: Rect = Rect::new(192.0, 205.1, 12.0, 12.0);
/// Voice lamp inside the input meter, lower left: lit while the voice
/// detector hears speech.
const VOICE_LAMP: Rect = Rect::new(41.0, 183.0, 8.0, 8.0);
const VOICE_LABEL_X: f32 = 53.0;
/// Hover area of the voice lamp and its label.
const VOICE_HIT: Rect = Rect::new(36.0, 176.0, 52.0, 22.0);
/// Microphone picker: the INPUT label row up to the peak lamp. Clicking it
/// switches to the next microphone; the current one is shown after INPUT.
const INPUT_PICK: Rect = Rect::new(30.0, 202.0, 156.0, 22.0);
const INPUT_NAME_X: f32 = 80.0;

const GR_LABEL: Rect = Rect::new(30.0, 236.6, 30.0, 17.0);
const GR_BAR: Rect = Rect::new(70.0, 236.1, 258.0, 18.0);
const GR_LED_Y: f32 = 241.1;
const GR_LED_W: f32 = 17.0;
const GR_LED_H: f32 = 8.0;
const GR_LED_PITCH: f32 = 21.0;
const GR_TEXT: Rect = Rect::new(338.0, 238.1, 52.0, 14.0);

const PRESET_Y: f32 = 268.1;
const PRESET_W: f32 = 67.2;
const PRESET_H: f32 = 52.0;
const PRESET_PITCH: f32 = 73.2;
const PRESET_LAMP_Y: f32 = 275.1;
const PRESET_LABEL_Y: f32 = 292.1;
/// What the selected preset is for, and its character gauges below.
const PRESET_DESC: Rect = Rect::new(30.0, 326.1, 360.0, 14.0);
/// The selected preset's character gauges, across the preset buttons' width.
const GAUGE_ROW: Rect = Rect::new(30.0, 350.0, 360.0, 12.0);
const GAUGE_H: f32 = 10.0;
const GAUGE_LED_W: f32 = 8.0;
const GAUGE_LED_GAP: f32 = 2.0;

const MON_BUTTON: Rect = Rect::new(50.0, 436.5 + DY, 44.0, 56.0);
const MON_PIVOT: Rect = Rect::new(66.0, 458.5 + DY, 12.0, 12.0);
const MON_LEVER_X: f32 = 69.0;
const MON_LEVER_OFF_Y: f32 = 465.5 + DY;
const MON_LEVER_ON_Y: f32 = 441.5 + DY;
const MON_LABEL: Rect = Rect::new(30.0, 498.5 + DY, 84.0, 16.0);
const MON_LAMP: Rect = Rect::new(66.0, 520.5 + DY, 12.0, 12.0);

pub const KNOB: Rect = Rect::new(150.0, 414.0 + DY, 120.0, 120.0);
const KNOB_LABEL: Rect = Rect::new(150.0, 538.0 + DY, 120.0, 17.0);

const AB_BUTTON: Rect = Rect::new(320.0, 436.0 + DY, 56.0, 56.0);
const AB_LABEL: Rect = Rect::new(306.0, 498.0 + DY, 84.0, 16.0);
const AB_HINT: Rect = Rect::new(306.0, 520.0 + DY, 84.0, 13.0);

const RULE_Y: f32 = 579.0 + DY;
const FOOTER_TEXT: Rect = Rect::new(30.0, 588.0 + DY, 200.0, 32.0);
const LINK: Rect = Rect::new(320.5, 588.0 + DY, 69.5, 32.0);
/// Calibration link, left of ADVANCED; shows the countdown while running.
const CAL_LINK: Rect = Rect::new(200.0, 588.0 + DY, 98.0, 32.0);

const KNOB_PIXELS_PER_UNIT: f32 = 2.0;

/// Noise removal row: "NOISE" label plus Off / Light / Medium / Strong.
const NOISE_Y: f32 = 354.1 + DY;
const NOISE_H: f32 = 36.0;
const NOISE_LABEL: Rect = Rect::new(30.0, 354.1 + DY, 64.0, 36.0);
const NOISE_GAP: f32 = 6.0;
const NOISE_BUTTON_X: f32 = 102.0;
const NOISE_BUTTON_W: f32 = (288.0 - 3.0 * NOISE_GAP) / 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Id {
    On,
    Mute,
    Preset(u8),
    /// 0 = off, 1..=3 = `DenoiseLevel::ALL[i - 1]`.
    Noise(u8),
    Monitor,
    Knob,
    Ab,
    Advanced,
    Calibrate,
    Input,
    Update,
}

pub struct StandardView {
    clicker: Clicker<Id>,
    /// Knob drag: pointer position and value at press time.
    drag: Option<(Point, u8)>,
    vu_in: VuBallistics,
    vu_out: VuBallistics,
    peak: PeakHold,
    voice: dsp::voice::SpeechGate,
    voice_lit: bool,
    gr: GrSmoother,
    lufs: Smoother,
    in_db: f32,
    out_db: f32,
    lufs_valid: bool,
    text: String,
    text2: String,
}

impl Default for StandardView {
    fn default() -> Self {
        Self::new()
    }
}

impl StandardView {
    pub fn new() -> Self {
        Self {
            clicker: Clicker::new(),
            drag: None,
            vu_in: VuBallistics::new(),
            vu_out: VuBallistics::new(),
            peak: PeakHold::new(PEAK_LAMP_DBFS, 0.5),
            voice: dsp::voice::SpeechGate::default(),
            voice_lit: false,
            gr: GrSmoother::default(),
            lufs: Smoother::new(-24.0, 0.4),
            in_db: -60.0,
            out_db: -60.0,
            lufs_valid: false,
            text: String::with_capacity(64),
            text2: String::with_capacity(64),
        }
    }

    fn hit(&self, p: Point, state: &UiState) -> Option<Id> {
        if self.mute_rect(state).contains(p) {
            return Some(Id::Mute);
        }
        if self.on_button_rect(state).contains(p) {
            return Some(Id::On);
        }
        for i in 0..5 {
            if preset_rect(i).contains(p) {
                return Some(Id::Preset(i as u8));
            }
        }
        for i in 0..4 {
            if noise_rect(i).contains(p) {
                return Some(Id::Noise(i as u8));
            }
        }
        if MON_BUTTON.contains(p) {
            return Some(Id::Monitor);
        }
        if KNOB.contains(p) {
            return Some(Id::Knob);
        }
        if AB_BUTTON.contains(p) {
            return Some(Id::Ab);
        }
        if LINK.contains(p) {
            return Some(Id::Advanced);
        }
        if CAL_LINK.contains(p) {
            return Some(Id::Calibrate);
        }
        if INPUT_PICK.contains(p) {
            return Some(Id::Input);
        }
        if state.update_available.is_some() && UPDATE_HIT.contains(p) {
            return Some(Id::Update);
        }
        None
    }

    fn mute_rect(&self, state: &UiState) -> Rect {
        let on = self.on_button_rect(state);
        Rect::new(on.x - 8.0 - MUTE_W, ON_BUTTON_Y, MUTE_W, ON_BUTTON_H)
    }

    fn on_button_rect(&self, state: &UiState) -> Rect {
        // 12 padding + 10 lamp + 8 gap + text + 12 padding; width measured
        // at paint time, here approximated from the longest label.
        let w = if state.on { 59.9 } else { 109.0 };
        Rect::new(CONTENT_RIGHT - w, ON_BUTTON_Y, w, ON_BUTTON_H)
    }

    fn activate(&self, id: Id, state: &UiState, out: &mut Vec<Action>) {
        match id {
            Id::On => out.push(Action::SetOn(!state.on)),
            Id::Mute => out.push(Action::SetMuted(!state.muted)),
            Id::Preset(i) => out.push(Action::SetPreset(PresetId::ALL[i as usize])),
            Id::Noise(0) => out.push(Action::SetNeuralDenoise(None)),
            Id::Noise(i) => out.push(Action::SetNeuralDenoise(Some(
                DenoiseLevel::ALL[i as usize - 1],
            ))),
            Id::Monitor => {
                let mut m = state.monitor;
                m.enabled = !m.enabled;
                out.push(Action::SetMonitor(m));
            }
            Id::Knob => {}
            Id::Ab => out.push(Action::SetAbOriginal(!state.ab_original)),
            Id::Advanced => out.push(Action::SwitchView(ViewKind::Advanced)),
            Id::Calibrate => out.push(Action::Recalibrate),
            Id::Input => out.push(Action::ChooseInput(INPUT_PICK)),
            Id::Update => out.push(Action::OpenUpdate),
        }
    }

    /// Dynamics gain reduction shown on the LED bar: compressor plus
    /// limiter, the two stages that act on the whole signal.
    pub fn total_gr_db(state: &UiState) -> f32 {
        let g = &state.meters.gain_reduction_db;
        g[dsp::Module::Compressor.index()].max(0.0) + g[dsp::Module::Limiter.index()].max(0.0)
    }
}

fn noise_rect(i: usize) -> Rect {
    Rect::new(
        NOISE_BUTTON_X + (NOISE_BUTTON_W + NOISE_GAP) * i as f32,
        NOISE_Y,
        NOISE_BUTTON_W,
        NOISE_H,
    )
}

/// Noise push button: 8 px lamp, 6 px gap, uppercase label, all centered.
/// A noise removal push button. The selected one looks pressed (darker
/// face, ink border, bolder label) and its lamp is lit, OFF included.
fn noise_button(p: &mut dyn Painter, r: Rect, label: &str, selected: bool) {
    let (face, border, width, weight, lamp_color) = if selected {
        (th::SWITCH_BODY, th::INK, 1.5, 600, th::AMBER)
    } else {
        (th::BUTTON, th::BUTTON_BORDER, 1.0, 400, th::LAMP_OFF)
    };
    p.fill_rounded_rect(r, 6.0, face);
    p.border_rounded_rect(r, 6.0, width, border);
    let style = TextStyle::new(Family::Oswald, weight, 11.0, th::INK).spacing(1.0);
    let tw = p.measure_text(label, &style).width;
    let x0 = r.x + (r.w - (10.0 + 6.0 + tw)) / 2.0;
    lamp(
        p,
        Rect::new(x0, r.y + (r.h - 10.0) / 2.0, 10.0, 10.0),
        lamp_color,
        true,
    );
    p.text_in_rect(
        label,
        Rect::new(x0 + 16.0, r.y, tw, r.h),
        &style,
        Align::Left,
    );
}

/// The selected preset's character as three small LED strips, in the
/// style of the gain reduction bar, spread across the preset buttons: the
/// first starts under BROADCAST, the last ends under NARRATION. They follow
/// the Intensity knob; the button tooltips say what each preset is for.
fn character_gauges(p: &mut dyn Painter, state: &UiState) {
    let levels = app_core::character_levels(state.preset, state.intensity);
    let label = TextStyle::new(Family::Oswald, 500, 10.0, th::INK_SOFT).spacing(1.5);
    let gauges = [
        ("COMPRESSION", levels.compression),
        ("BRIGHT", levels.brightness),
        ("WARM", levels.warmth),
    ];
    let strip_w = 5.0 * GAUGE_LED_W + 4.0 * GAUGE_LED_GAP + 4.0;
    let widths = gauges.map(|(name, _)| p.measure_text(name, &label).width + 6.0 + strip_w);
    let gap = (GAUGE_ROW.w - widths.iter().sum::<f32>()) / 2.0;
    let mut x = GAUGE_ROW.x;
    for ((name, lit), w) in gauges.iter().zip(widths) {
        let lw = w - 6.0 - strip_w;
        p.text_in_rect(
            name,
            Rect::new(x, GAUGE_ROW.y, lw, GAUGE_ROW.h),
            &label,
            Align::Left,
        );
        let strip = Rect::new(
            x + lw + 6.0,
            GAUGE_ROW.y + (GAUGE_ROW.h - GAUGE_H) / 2.0,
            strip_w,
            GAUGE_H,
        );
        p.fill_rounded_rect(strip, 2.5, th::INK);
        for i in 0..5u8 {
            p.fill_rounded_rect(
                Rect::new(
                    strip.x + 2.0 + (GAUGE_LED_W + GAUGE_LED_GAP) * i as f32,
                    strip.y + 2.0,
                    GAUGE_LED_W,
                    GAUGE_H - 4.0,
                ),
                1.0,
                if i < *lit { th::LED_LOW } else { th::LED_OFF },
            );
        }
        x += w + gap;
    }
}

fn preset_rect(i: usize) -> Rect {
    Rect::new(30.0 + PRESET_PITCH * i as f32, PRESET_Y, PRESET_W, PRESET_H)
}

fn lamp(p: &mut dyn Painter, r: Rect, fill: Color, border: bool) {
    let c = r.center();
    p.fill_circle(c.x, c.y, r.w / 2.0, fill);
    if border {
        p.begin_path();
        p.circle(c.x, c.y, r.w / 2.0 - 0.5);
        p.stroke(Stroke::new(th::INK, 1.0));
    }
}

impl View for StandardView {
    fn preferred_size(&self) -> (f32, f32) {
        (WIDTH, HEIGHT)
    }

    fn layout(&mut self, _w: f32, _h: f32, _state: &UiState, _p: &mut dyn Painter) {}

    fn static_key(&self) -> u64 {
        1
    }

    fn tick(&mut self, state: &UiState, dt: f32) {
        let m = &state.meters;
        let (vu_in, vu_out) = vu_inputs_db(m);
        self.in_db = self.vu_in.update(vu_in, dt);
        self.out_db = self.vu_out.update(vu_out, dt);
        self.peak.update(m.in_peak_db, dt);
        self.voice_lit = m.speech_probability.is_some_and(|p| self.voice.update(p));
        self.gr.update(Self::total_gr_db(state), dt);
        // No loudness below the -70 LUFS gate (silence, muted): show --.-.
        self.lufs_valid = m.lufs_short_term.is_finite();
        if self.lufs_valid {
            self.lufs.update(m.lufs_short_term, dt);
        }
    }

    fn paint_static(&mut self, p: &mut dyn Painter) {
        p.fill_rounded_rect(Rect::new(0.0, 0.0, WIDTH, HEIGHT), 16.0, th::FRAME);
        p.fill_rounded_rect(FACEPLATE, 10.0, th::FACEPLATE);
        for (x, y) in SCREWS {
            p.fill_circle(x + 3.0, y + 3.0, 3.0, th::SCREW);
        }

        let title = TextStyle::new(Family::Oswald, 600, 20.0, th::INK).spacing(3.0);
        p.fill_text("VOICE STRIP", TITLE.x, TITLE.y, &title, Align::Left);
        let sub = TextStyle::new(Family::Oswald, 400, 11.0, th::INK_SOFT).spacing(2.0);
        p.fill_text("VOICE PROCESSOR", SUBTITLE.x, SUBTITLE.y, &sub, Align::Left);

        for r in [VU_IN, VU_OUT] {
            p.save();
            p.translate(r.x, r.y);
            p.scale(r.w / vu::VIEWBOX_W, r.h / vu::VIEWBOX_H);
            vu::paint_face(p);
            p.restore();
        }
        let label = TextStyle::new(Family::Oswald, 400, 12.0, th::INK).spacing(2.0);
        p.fill_text("INPUT", VU_IN.x, VU_LABEL_Y, &label, Align::Left);
        p.fill_text("OUTPUT", VU_OUT.x, VU_LABEL_Y, &label, Align::Left);

        p.text_in_rect("GR", GR_LABEL, &label, Align::Left);
        p.fill_rounded_rect(GR_BAR, 4.0, th::INK);

        // Knob body and labels of the bottom row.
        p.save();
        p.translate(KNOB.x, KNOB.y);
        knob::paint_body(p);
        p.restore();
        let small = TextStyle::new(Family::Oswald, 400, 11.0, th::INK).spacing(2.0);
        p.text_in_rect("MONITOR", MON_LABEL, &small, Align::Center);
        p.text_in_rect("A / B", AB_LABEL, &small, Align::Center);

        p.fill_rect(Rect::new(30.0, RULE_Y, 360.0, 1.0), th::RULE);
    }

    fn paint_dynamic(&mut self, p: &mut dyn Painter, state: &UiState) {
        // ON / BYPASS button.
        let on_style = TextStyle::new(Family::Oswald, 400, 13.0, th::FACEPLATE).spacing(2.0);
        let on_label = if state.on { "ON" } else { "BYPASS" };
        let tw = p.measure_text(on_label, &on_style).width;
        let bw = 12.0 + 10.0 + 8.0 + tw + 12.0;
        let btn = Rect::new(CONTENT_RIGHT - bw, ON_BUTTON_Y, bw, ON_BUTTON_H);
        p.fill_rounded_rect(btn, 6.0, th::INK);
        let lamp_r = Rect::new(btn.x + 12.0, btn.y + 17.0, 10.0, 10.0);
        lamp(
            p,
            lamp_r,
            if state.on { th::AMBER } else { th::INK_SOFT },
            false,
        );
        p.text_in_rect(
            on_label,
            Rect::new(btn.x + 30.0, btn.y, tw, btn.h),
            &on_style,
            Align::Left,
        );

        // MUTE button: faceplate-coloured, red with a lit lamp when muted.
        let muted = state.muted;
        let mute_btn = self.mute_rect(state);
        let hover = self.clicker.hover == Some(Id::Mute);
        let (face, border, text_color, lamp_color) = if muted {
            (th::RED, th::RED, th::FACEPLATE, th::PEAK_LIT_BRIGHT)
        } else {
            (
                th::BUTTON,
                if hover { th::INK } else { th::BUTTON_BORDER },
                th::INK,
                th::LAMP_OFF,
            )
        };
        p.fill_rounded_rect(mute_btn, 6.0, face);
        p.border_rounded_rect(mute_btn, 6.0, 1.0, border);
        let mute_style = TextStyle::new(
            Family::Oswald,
            if muted { 600 } else { 400 },
            13.0,
            text_color,
        )
        .spacing(2.0);
        let mute_label = if muted { "MUTED" } else { "MUTE" };
        let mtw = p.measure_text(mute_label, &mute_style).width;
        let mx = mute_btn.x + (mute_btn.w - (10.0 + 8.0 + mtw)) / 2.0;
        lamp(
            p,
            Rect::new(mx, mute_btn.y + 17.0, 10.0, 10.0),
            lamp_color,
            !muted,
        );
        p.text_in_rect(
            mute_label,
            Rect::new(mx + 18.0, mute_btn.y, mtw, mute_btn.h),
            &mute_style,
            Align::Left,
        );

        // A newer release: the subtitle becomes a lamp and UPDATE x.y.z.
        if let Some(v) = &state.update_available {
            p.fill_rect(
                Rect::new(
                    SUBTITLE.x - 2.0,
                    SUBTITLE.y - 2.0,
                    SUBTITLE.w + 30.0,
                    SUBTITLE.h + 4.0,
                ),
                th::FACEPLATE,
            );
            lamp(
                p,
                Rect::new(SUBTITLE.x, SUBTITLE.y + 4.0, 8.0, 8.0),
                th::AMBER,
                true,
            );
            let hover = self.clicker.hover == Some(Id::Update);
            let style = TextStyle::new(
                Family::Oswald,
                500,
                11.0,
                if hover { th::LINK_HOVER } else { th::INK },
            )
            .spacing(2.0);
            self.text2.clear();
            let _ = write!(self.text2, "UPDATE {v}");
            p.fill_text(
                &self.text2,
                SUBTITLE.x + 14.0,
                SUBTITLE.y,
                &style,
                Align::Left,
            );
        }

        // Needles.
        for (r, db) in [(VU_IN, self.in_db), (VU_OUT, self.out_db)] {
            p.save();
            p.translate(r.x, r.y);
            p.scale(r.w / vu::VIEWBOX_W, r.h / vu::VIEWBOX_H);
            p.clip_rounded_rect(Rect::new(1.0, 1.0, 198.0, 128.0), 8.0);
            vu::paint_needle(p, db);
            p.restore();
        }
        lamp(
            p,
            PEAK_LAMP,
            if self.peak.lit() {
                th::PEAK_LIT
            } else {
                th::LAMP_OFF
            },
            true,
        );
        lamp(
            p,
            VOICE_LAMP,
            if self.voice_lit {
                th::AMBER
            } else {
                th::LAMP_OFF
            },
            true,
        );
        let voice_style = TextStyle::new(
            Family::Oswald,
            400,
            9.0,
            if self.voice_lit {
                th::INK
            } else {
                th::INK_SOFT
            },
        )
        .spacing(1.5);
        p.fill_text(
            "VOICE",
            VOICE_LABEL_X,
            VOICE_LAMP.y - 2.5,
            &voice_style,
            Align::Left,
        );
        let mono11 = TextStyle::new(Family::PlexMono, 400, 11.0, th::INK_SOFT);
        self.text.clear();
        if self.lufs_valid {
            let _ = write!(self.text, "{:.1} LUFS", self.lufs.value());
        } else {
            self.text.push_str("--.- LUFS");
        }
        p.fill_text(&self.text, CONTENT_RIGHT, VU_LABEL_Y, &mono11, Align::Right);
        let _ = VU_LABEL_H;

        // Microphone picker: current device after INPUT, with a caret.
        let hover = self.clicker.hover == Some(Id::Input);
        let name_style = TextStyle::new(
            Family::PlexMono,
            400,
            10.0,
            if hover { th::LINK_HOVER } else { th::INK_SOFT },
        );
        let name = if state.input_device.is_empty() {
            "Default mic"
        } else {
            state.input_device.as_str()
        };
        let caret_w = 10.0;
        let max_w = PEAK_LAMP.x - 6.0 - INPUT_NAME_X - caret_w;
        self.text.clear();
        self.text.push_str(name);
        if p.measure_text(&self.text, &name_style).width > max_w {
            while !self.text.is_empty()
                && p.measure_text(&format!("{}…", self.text), &name_style)
                    .width
                    > max_w
            {
                self.text.pop();
            }
            self.text.push('…');
        }
        let w = p.measure_text(&self.text, &name_style).width;
        p.fill_text(
            &self.text,
            INPUT_NAME_X,
            VU_LABEL_Y + 2.0,
            &name_style,
            Align::Left,
        );
        let cx = INPUT_NAME_X + w + 6.0;
        let cy = VU_LABEL_Y + 8.0;
        let caret = Stroke::new(name_style.color, 1.2).cap(LineCap::Round);
        p.line(cx - 3.0, cy - 1.5, cx, cy + 1.5, caret);
        p.line(cx, cy + 1.5, cx + 3.0, cy - 1.5, caret);

        // Gain reduction LEDs.
        let gr = self.gr.value().min(12.0);
        let lit = gr.round() as usize;
        for i in 0..12 {
            let color = if i >= lit {
                th::LED_OFF
            } else if i < 6 {
                th::LED_LOW
            } else if i < 9 {
                th::LED_MID
            } else {
                th::LED_HIGH
            };
            p.fill_rounded_rect(
                Rect::new(
                    GR_BAR.x + 5.0 + GR_LED_PITCH * i as f32,
                    GR_LED_Y,
                    GR_LED_W,
                    GR_LED_H,
                ),
                2.0,
                color,
            );
        }
        self.text.clear();
        if gr > 0.05 {
            let _ = write!(self.text, "-{:.1} dB", gr);
        } else {
            self.text.push_str("0.0 dB");
        }
        p.fill_text(
            &self.text,
            GR_TEXT.right(),
            GR_TEXT.y,
            &mono11.color(th::INK),
            Align::Right,
        );

        // Presets.
        let preset_style = TextStyle::new(Family::Oswald, 400, 11.0, th::INK).spacing(1.0);
        for (i, id) in PresetId::ALL.iter().enumerate() {
            let r = preset_rect(i);
            let active = *id == state.preset;
            p.fill_rounded_rect(r, 6.0, th::BUTTON);
            p.border_rounded_rect(r, 6.0, 1.0, th::BUTTON_BORDER);
            lamp(
                p,
                Rect::new(r.x + (r.w - 12.0) / 2.0, PRESET_LAMP_Y, 12.0, 12.0),
                if active { th::AMBER } else { th::LAMP_OFF },
                true,
            );
            self.text.clear();
            for ch in builtin(*id).label.chars() {
                self.text.extend(ch.to_uppercase());
            }
            p.fill_text(
                &self.text,
                r.x + r.w / 2.0,
                PRESET_LABEL_Y,
                &preset_style,
                Align::Center,
            );
        }
        p.fill_text(
            builtin(state.preset).description,
            PRESET_DESC.x + PRESET_DESC.w / 2.0,
            PRESET_DESC.y,
            &mono11,
            Align::Center,
        );
        character_gauges(p, state);

        // Noise removal: label (with the model download size while the
        // model is missing), Off plus the three levels.
        let label = TextStyle::new(Family::Oswald, 400, 12.0, th::INK).spacing(2.0);
        if state.model == ModelState::Ready {
            p.text_in_rect("NOISE", NOISE_LABEL, &label, Align::Left);
        } else {
            let (top, bottom) = (
                Rect::new(NOISE_LABEL.x, NOISE_LABEL.y, NOISE_LABEL.w, 20.0),
                Rect::new(NOISE_LABEL.x, NOISE_LABEL.y + 20.0, NOISE_LABEL.w, 14.0),
            );
            p.text_in_rect("NOISE", top, &label, Align::Left);
            download_mark(p, bottom.x, bottom.y + 2.5, th::INK_SOFT);
            let size = TextStyle::new(Family::PlexMono, 400, 10.0, th::INK_SOFT);
            p.text_in_rect(
                "35 MB",
                Rect::new(bottom.x + 11.0, bottom.y, bottom.w - 11.0, bottom.h),
                &size,
                Align::Left,
            );
        }
        let denoise = state.neural_denoise;
        noise_button(p, noise_rect(0), "OFF", denoise.is_none());
        for (i, level) in DenoiseLevel::ALL.iter().enumerate() {
            self.text.clear();
            for ch in level.label().chars() {
                self.text.extend(ch.to_uppercase());
            }
            noise_button(p, noise_rect(i + 1), &self.text, denoise == Some(*level));
        }

        // Headphone monitoring lever.
        let mon = state.monitor.enabled;
        p.fill_rounded_rect(MON_BUTTON, 6.0, th::SWITCH_BODY);
        p.border_rounded_rect(MON_BUTTON, 6.0, 1.0, th::BUTTON_BORDER);
        lamp(p, MON_PIVOT, th::SWITCH_PIVOT, false);
        let lever_y = if mon { MON_LEVER_ON_Y } else { MON_LEVER_OFF_Y };
        p.fill_rounded_rect(Rect::new(MON_LEVER_X, lever_y, 6.0, 22.0), 3.0, th::INK);
        lamp(
            p,
            MON_LAMP,
            if mon { th::MONITOR_LIT } else { th::LAMP_OFF },
            true,
        );

        // Knob pointer and value.
        p.save();
        p.translate(KNOB.x, KNOB.y);
        knob::paint_pointer(p, state.intensity);
        p.restore();
        let klabel = TextStyle::new(Family::Oswald, 400, 12.0, th::INK).spacing(2.0);
        let kvalue = TextStyle::new(Family::PlexMono, 400, 12.0, th::INK);
        self.text.clear();
        let _ = write!(self.text, "{}", state.intensity);
        let lw = p.measure_text("INTENSITY ", &klabel).width;
        let vw = p.measure_text(&self.text, &kvalue).width;
        let x0 = KNOB_LABEL.x + (KNOB_LABEL.w - lw - vw) / 2.0;
        p.text_in_rect(
            "INTENSITY ",
            Rect::new(x0, KNOB_LABEL.y, lw, KNOB_LABEL.h),
            &klabel,
            Align::Left,
        );
        p.text_in_rect(
            &self.text,
            Rect::new(x0 + lw, KNOB_LABEL.y, vw, KNOB_LABEL.h),
            &kvalue,
            Align::Left,
        );

        // A/B button.
        p.fill_rounded_rect(AB_BUTTON, 28.0, th::BUTTON);
        p.border_rounded_rect(AB_BUTTON, 28.0, 2.0, th::INK);
        let ab_style = TextStyle::new(Family::Oswald, 600, 15.0, th::INK).spacing(1.0);
        p.text_in_rect(
            if state.ab_original { "B" } else { "A" },
            AB_BUTTON,
            &ab_style,
            Align::Center,
        );
        let hint = TextStyle::new(Family::PlexMono, 400, 10.0, th::INK_SOFT);
        p.fill_text(
            if state.ab_original {
                "original"
            } else {
                "processed"
            },
            AB_HINT.x + AB_HINT.w / 2.0,
            AB_HINT.y,
            &hint,
            Align::Center,
        );

        // Footer.
        let foot = TextStyle::new(Family::PlexMono, 400, 12.0, th::INK_SOFT);
        self.text2.clear();
        if let Some(notice) = &state.notice {
            // A problem the user must see replaces the latency line, on up
            // to two lines left of CALIBRATE; the rest is cut.
            let warn = TextStyle::new(Family::PlexMono, 400, 10.0, th::RED);
            let width = NOTICE_RIGHT - FOOTER_TEXT.x;
            let mut lines = [String::new(), String::new()];
            let mut line = 0;
            for word in notice.split_whitespace() {
                let candidate = if lines[line].is_empty() {
                    word.to_string()
                } else {
                    format!("{} {word}", lines[line])
                };
                if p.measure_text(&candidate, &warn).width <= width || lines[line].is_empty() {
                    lines[line] = candidate;
                } else if line == 0 {
                    line = 1;
                    lines[1] = word.to_string();
                } else {
                    lines[1].push('…');
                    break;
                }
            }
            for (i, l) in lines.iter().enumerate() {
                p.fill_text(
                    l,
                    FOOTER_TEXT.x,
                    FOOTER_TEXT.y + 3.0 + 13.0 * i as f32,
                    &warn,
                    Align::Left,
                );
            }
        } else {
            // Short form with monitoring on, to leave room for CALIBRATE.
            if mon {
                let _ = write!(
                    self.text2,
                    "MIC {:.1} · MON {:.1} ms",
                    state.mic_latency_ms, state.monitor_latency_ms
                );
            } else {
                let _ = write!(self.text2, "MIC {:.1} ms", state.mic_latency_ms);
            }
            p.text_in_rect(&self.text2, FOOTER_TEXT, &foot, Align::Left);
        }
        let link_color = if self.clicker.hover == Some(Id::Advanced) {
            th::LINK_HOVER
        } else {
            th::INK
        };
        let link = TextStyle::new(Family::Oswald, 500, 12.0, link_color).spacing(2.0);
        p.text_in_rect(
            "ADVANCED",
            Rect::new(LINK.x, LINK.y, LINK.w - 4.0, LINK.h),
            &link,
            Align::Right,
        );

        // Calibration: link when idle, countdown while running, outcome after.
        self.text2.clear();
        let mut cal_color = if self.clicker.hover == Some(Id::Calibrate) {
            th::LINK_HOVER
        } else {
            th::INK
        };
        match &state.calibration {
            None => self.text2.push_str("CALIBRATE"),
            Some(c) => match c.phase {
                app_core::calibration::Phase::Silence => {
                    let _ = write!(self.text2, "QUIET {:.0}", c.seconds_left.ceil());
                    cal_color = th::LINK_HOVER;
                }
                app_core::calibration::Phase::Speech => {
                    let _ = write!(self.text2, "SPEAK {:.0}", c.seconds_left.ceil());
                    cal_color = th::LINK_HOVER;
                }
                app_core::calibration::Phase::Done if c.error => {
                    self.text2.push_str("RETRY");
                    cal_color = th::RED;
                }
                app_core::calibration::Phase::Done => self.text2.push_str("CALIBRATED"),
            },
        }
        let cal = TextStyle::new(Family::Oswald, 500, 12.0, cal_color).spacing(2.0);
        p.text_in_rect(&self.text2, CAL_LINK, &cal, Align::Right);
        // Not calibrated yet: an amber lamp before the link asks for it.
        if !state.calibrated && state.calibration.is_none() {
            let w = p.measure_text(&self.text2, &cal).width;
            let r = Rect::new(
                CAL_LINK.x + CAL_LINK.w - w - 16.0,
                CAL_LINK.y + (CAL_LINK.h - 8.0) / 2.0,
                8.0,
                8.0,
            );
            lamp(p, r, th::AMBER, true);
        }
    }

    fn handle(&mut self, ev: Event, state: &UiState, out: &mut Vec<Action>) {
        match ev {
            Event::PointerMove(pt) => {
                if let Some((start, value)) = self.drag {
                    let v = knob::drag_value(
                        value,
                        pt.x - start.x,
                        pt.y - start.y,
                        KNOB_PIXELS_PER_UNIT,
                    );
                    if v != state.intensity {
                        out.push(Action::SetIntensity(v));
                    }
                } else {
                    self.clicker.moved(self.hit(pt, state));
                }
            }
            Event::PointerDown(pt) => {
                let hit = self.hit(pt, state);
                self.clicker.down(hit);
                if hit == Some(Id::Knob) {
                    self.drag = Some((pt, state.intensity));
                }
            }
            Event::PointerUp(pt) => {
                self.drag = None;
                if let Some(id) = self.clicker.up(self.hit(pt, state)) {
                    self.activate(id, state, out);
                }
            }
            Event::PointerLeave => {
                self.drag = None;
                self.clicker.leave();
            }
            Event::Scroll(pt, notches) => {
                if KNOB.contains(pt) {
                    let v = knob::scroll_value(state.intensity, notches);
                    if v != state.intensity {
                        out.push(Action::SetIntensity(v));
                    }
                }
            }
            Event::Key(k) => match k {
                Key::Up | Key::Right => out.push(Action::SetIntensity(
                    state.intensity.saturating_add(1).min(100),
                )),
                Key::Down | Key::Left => {
                    out.push(Action::SetIntensity(state.intensity.saturating_sub(1)))
                }
                Key::Char(c) => match c {
                    '1'..='5' => {
                        let i = c as usize - '1' as usize;
                        out.push(Action::SetPreset(PresetId::ALL[i]));
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
                _ => {}
            },
        }
    }

    fn tooltip(&self, p: Point, state: &UiState) -> Option<String> {
        use crate::help;
        if let Some(id) = self.hit(p, state) {
            return Some(match id {
                Id::On => help::on_switch(state),
                Id::Mute => help::mute(state),
                Id::Preset(i) => help::preset(PresetId::ALL[i as usize], state),
                Id::Noise(i) => help::noise(
                    if i == 0 {
                        None
                    } else {
                        Some(DenoiseLevel::ALL[i as usize - 1])
                    },
                    state,
                ),
                Id::Monitor => help::monitor(state),
                Id::Knob => help::intensity(state),
                Id::Ab => help::ab(state),
                Id::Advanced => help::advanced(),
                Id::Calibrate => help::calibrate(state),
                Id::Update => help::update(state),
                Id::Input => help::device_input(state),
            });
        }
        if VOICE_HIT.contains(p) {
            Some(help::models(state))
        } else if VU_IN.contains(p) {
            Some(help::vu_input())
        } else if VU_OUT.contains(p) {
            Some(help::vu_output(state))
        } else if GR_BAR.contains(p) {
            Some(help::gain_reduction())
        } else {
            None
        }
    }

    fn cursor(&self) -> Cursor {
        match self.clicker.hover {
            Some(Id::Knob) => Cursor::EwResize,
            Some(_) => Cursor::Pointer,
            None => Cursor::Default,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> UiState {
        let mut s = crate::demo::demo_state();
        s.on = true;
        s
    }

    #[test]
    fn hit_testing() {
        let v = StandardView::new();
        let s = state();
        assert_eq!(v.hit(Point::new(360.0, 50.0), &s), Some(Id::On));
        assert_eq!(v.hit(Point::new(40.0, 290.0), &s), Some(Id::Preset(0)));
        assert_eq!(v.hit(Point::new(330.0, 290.0), &s), Some(Id::Preset(4)));
        assert_eq!(v.hit(Point::new(72.0, 473.0), &s), Some(Id::Monitor));
        assert_eq!(v.hit(Point::new(110.0, 394.0), &s), Some(Id::Noise(0)));
        assert_eq!(v.hit(Point::new(380.0, 394.0), &s), Some(Id::Noise(3)));
        assert_eq!(v.hit(Point::new(210.0, 498.0), &s), Some(Id::Knob));
        assert_eq!(v.hit(Point::new(348.0, 488.0), &s), Some(Id::Ab));
        assert_eq!(v.hit(Point::new(370.0, 628.0), &s), Some(Id::Advanced));
        assert_eq!(v.hit(Point::new(210.0, 150.0), &s), None);
    }

    #[test]
    fn click_emits_actions() {
        let mut v = StandardView::new();
        let s = state();
        let mut out = Vec::new();
        v.handle(Event::PointerDown(Point::new(40.0, 290.0)), &s, &mut out);
        v.handle(Event::PointerUp(Point::new(40.0, 290.0)), &s, &mut out);
        assert_eq!(out, vec![Action::SetPreset(PresetId::Broadcast)]);
        out.clear();
        v.handle(Event::PointerDown(Point::new(360.0, 50.0)), &s, &mut out);
        v.handle(Event::PointerUp(Point::new(200.0, 50.0)), &s, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn knob_drag_sets_intensity() {
        let mut v = StandardView::new();
        let s = state();
        let mut out = Vec::new();
        v.handle(Event::PointerDown(Point::new(210.0, 498.0)), &s, &mut out);
        v.handle(Event::PointerMove(Point::new(230.0, 498.0)), &s, &mut out);
        assert_eq!(out, vec![Action::SetIntensity(s.intensity + 10)]);
        v.handle(Event::PointerUp(Point::new(230.0, 498.0)), &s, &mut out);
        assert_eq!(out.len(), 1);
    }
}
