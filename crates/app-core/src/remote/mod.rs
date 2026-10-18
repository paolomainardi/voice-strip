//! Remote control protocol, shared by every frontend: desktop bars, key
//! bindings and scripts drive the running app with one text command per
//! line and read its state back as one JSON object per line.
//!
//! ```text
//! status                         state, with the preset list
//! watch                          state now, then on every change and
//!                                with live meters about 30 times a second
//! on | off | toggle              processing (off is bypass)
//! mute on | off | toggle         others hear silence
//! preset <id>                    broadcast podcast call stream narration
//! noise <level>                  off light medium strong
//! intensity <0-100>
//! phones on | off | toggle       headphone monitor
//! model download                 download the noise removal model, which
//!                                `noise` needs
//! show                           open the window, or bring it to the front
//! start                          nothing for a running app; the client
//!                                starts a closed one without its window
//! quit
//! ```
//!
//! Every command answers with the state after it was applied, or with
//! `{"error":"..."}`.
//!
//! The protocol, the socket [`server`] and the command-line [`client`] are
//! toolkit-neutral. A frontend starts the server, calls
//! [`server::Server::poll`] from its UI loop, applies the commands with
//! [`apply`] and handles the two it owns (show and quit).

use std::fmt::Write;

use crate::calibration::Phase;
use crate::controller::{Controller, DetectorState, ModelState, UiState};
use crate::presets::{DenoiseLevel, PresetId, builtin};

#[cfg(unix)]
pub mod client;
#[cfg(unix)]
pub mod server;

/// The longest command line a client may send, bytes.
pub const MAX_LINE: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Status,
    Watch,
    SetOn(bool),
    ToggleOn,
    SetMuted(bool),
    ToggleMute,
    SetPreset(PresetId),
    SetNoise(Option<DenoiseLevel>),
    SetIntensity(u8),
    SetPhones(bool),
    TogglePhones,
    DownloadModel,
    Show,
    Start,
    Quit,
}

impl Command {
    /// Parses one command line. The error is the message sent back.
    pub fn parse(line: &str) -> Result<Self, String> {
        let mut words = line.split_whitespace();
        let name = words.next().unwrap_or("");
        let arg = words.next();
        if words.next().is_some() {
            return Err(format!("too many arguments: {}", line.trim()));
        }
        let need = |what: &str| arg.ok_or_else(|| format!("{name} needs {what}"));
        let none = |c: Command| match arg {
            None => Ok(c),
            Some(a) => Err(format!("{name} takes no argument, got {a}")),
        };
        match name {
            "status" => none(Command::Status),
            "watch" => none(Command::Watch),
            "on" => none(Command::SetOn(true)),
            "off" => none(Command::SetOn(false)),
            "toggle" => none(Command::ToggleOn),
            "preset" => {
                let a = need("a preset")?;
                preset_from_id(a)
                    .map(Command::SetPreset)
                    .ok_or_else(|| format!("unknown preset: {a}"))
            }
            "noise" => {
                let a = need("a level")?;
                noise_from_id(a)
                    .map(Command::SetNoise)
                    .ok_or_else(|| format!("unknown noise level: {a}"))
            }
            "intensity" => {
                let a = need("a value from 0 to 100")?;
                match a.parse::<u8>() {
                    Ok(v) if v <= 100 => Ok(Command::SetIntensity(v)),
                    _ => Err(format!("intensity must be 0 to 100, got {a}")),
                }
            }
            "mute" => match need("on, off or toggle")? {
                "on" => Ok(Command::SetMuted(true)),
                "off" => Ok(Command::SetMuted(false)),
                "toggle" => Ok(Command::ToggleMute),
                a => Err(format!("mute takes on, off or toggle, got {a}")),
            },
            "phones" => match need("on, off or toggle")? {
                "on" => Ok(Command::SetPhones(true)),
                "off" => Ok(Command::SetPhones(false)),
                "toggle" => Ok(Command::TogglePhones),
                a => Err(format!("phones takes on, off or toggle, got {a}")),
            },
            "model" => match need("download")? {
                "download" => Ok(Command::DownloadModel),
                a => Err(format!("model takes download, got {a}")),
            },
            "show" => none(Command::Show),
            "start" => none(Command::Start),
            "quit" => none(Command::Quit),
            "" => Err("empty command".into()),
            _ => Err(format!("unknown command: {name}")),
        }
    }
}

/// What the frontend still has to do after [`apply`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Bring the window to the front.
    Show,
    Quit,
}

/// Applies a command to the controller. Status and watch change nothing.
/// The error is the message sent back.
pub fn apply(controller: &mut Controller, command: Command) -> Result<Effect, String> {
    match command {
        Command::Status | Command::Watch | Command::Start => {}
        Command::SetOn(on) => controller.set_on(on),
        Command::ToggleOn => {
            let on = !controller.state().on;
            controller.set_on(on);
        }
        Command::SetMuted(m) => controller.set_muted(m),
        Command::ToggleMute => {
            let m = !controller.state().muted;
            controller.set_muted(m);
        }
        Command::SetPreset(p) => controller.set_preset(p),
        Command::SetNoise(Some(_)) if !controller.model_ready() => {
            return Err(
                "noise removal needs its model (34.7 MB): run `voice-strip ctl model download` \
                 first"
                    .into(),
            );
        }
        Command::SetNoise(n) => controller.set_neural_denoise(n),
        Command::DownloadModel => controller.download_model(),
        Command::SetIntensity(i) => controller.set_intensity(i),
        Command::SetPhones(on) => set_phones(controller, on),
        Command::TogglePhones => {
            let on = !controller.state().monitor.enabled;
            set_phones(controller, on);
        }
        Command::Show => return Ok(Effect::Show),
        Command::Quit => return Ok(Effect::Quit),
    }
    Ok(Effect::None)
}

fn set_phones(controller: &mut Controller, on: bool) {
    let mut m = controller.state().monitor;
    if m.enabled != on {
        m.enabled = on;
        controller.set_monitor(m);
    }
}

/// Command-line name of a preset.
pub fn preset_id(p: PresetId) -> &'static str {
    match p {
        PresetId::Broadcast => "broadcast",
        PresetId::Podcast => "podcast",
        PresetId::Call => "call",
        PresetId::Stream => "stream",
        PresetId::Narration => "narration",
    }
}

pub fn preset_from_id(id: &str) -> Option<PresetId> {
    PresetId::ALL.into_iter().find(|p| preset_id(*p) == id)
}

/// Command-line name of a noise removal level; `off` for `None`.
pub fn noise_id(level: Option<DenoiseLevel>) -> &'static str {
    match level {
        None => "off",
        Some(DenoiseLevel::Light) => "light",
        Some(DenoiseLevel::Medium) => "medium",
        Some(DenoiseLevel::Strong) => "strong",
    }
}

pub fn noise_from_id(id: &str) -> Option<Option<DenoiseLevel>> {
    if id == "off" {
        return Some(None);
    }
    DenoiseLevel::ALL
        .into_iter()
        .find(|l| noise_id(Some(*l)) == id)
        .map(Some)
}

/// Meters as the window shows them, after ballistics: VU levels relative to
/// the VU reference, gain reduction of the dynamics stages, short-term
/// loudness and the peak lamp. The VU inputs before ballistics come along,
/// so a client can run the same needle movement at its own frame rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayMeters {
    pub in_vu_db: f32,
    pub out_vu_db: f32,
    /// Input VU level before ballistics, dB relative to the VU reference.
    pub in_level_db: f32,
    /// Output VU level before ballistics.
    pub out_level_db: f32,
    pub gr_db: f32,
    /// `None` until the loudness meter has a reading.
    pub lufs: Option<f32>,
    pub peak: bool,
}

impl Default for DisplayMeters {
    /// Needles at rest, before the first reading.
    fn default() -> Self {
        Self {
            in_vu_db: -60.0,
            out_vu_db: -60.0,
            in_level_db: f32::NEG_INFINITY,
            out_level_db: f32::NEG_INFINITY,
            gr_db: 0.0,
            lufs: None,
            peak: false,
        }
    }
}

/// What [`state_json`] includes beyond the state itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct Extras<'a> {
    pub meters: Option<&'a DisplayMeters>,
    /// The preset list with labels and descriptions, for clients that
    /// build their buttons from it.
    pub presets: bool,
    /// Folder of the bundled fonts, so clients can draw with the same type.
    pub fonts_dir: Option<&'a str>,
}

/// The state as one JSON object, without a trailing newline.
pub fn state_json(st: &UiState, extras: Extras) -> String {
    let mut s = String::with_capacity(512);
    let preset = builtin(st.preset);
    s.push_str("{\"running\":true");
    let _ = write!(s, ",\"on\":{}", st.on);
    let _ = write!(s, ",\"muted\":{}", st.muted);
    let _ = write!(s, ",\"preset\":\"{}\"", preset_id(st.preset));
    s.push_str(",\"preset_label\":");
    push_str(&mut s, preset.label);
    s.push_str(",\"preset_description\":");
    push_str(&mut s, preset.description);
    let _ = write!(s, ",\"noise\":\"{}\"", noise_id(st.neural_denoise));
    let model = match st.model {
        ModelState::Missing => "missing",
        ModelState::Downloading(_) => "downloading",
        ModelState::Failed(_) => "failed",
        ModelState::Ready => "ready",
    };
    let _ = write!(s, ",\"model\":\"{model}\"");
    let detector = match st.voice_detector {
        DetectorState::Loading => "loading",
        DetectorState::Running => "running",
        DetectorState::Failed(_) => "failed",
    };
    let _ = write!(s, ",\"voice_detector\":\"{detector}\"");
    let opt = |s: &mut String, v: Option<f32>| match v {
        Some(v) => push_num(s, v),
        None => s.push_str("null"),
    };
    s.push_str(",\"speech\":");
    opt(&mut s, st.meters.speech_probability);
    s.push_str(",\"room_db\":");
    opt(&mut s, st.meters.room_db);
    let _ = write!(s, ",\"intensity\":{}", st.intensity);
    let _ = write!(s, ",\"phones\":{}", st.monitor.enabled);
    s.push_str(",\"input\":");
    push_str(&mut s, &st.input_device);
    s.push_str(",\"virtual_mic\":");
    push_str(&mut s, &st.virtual_device);
    s.push_str(",\"mic_latency_ms\":");
    push_num(&mut s, st.mic_latency_ms);
    // The status stays after a calibration ends, with its outcome.
    let calibrating = st
        .calibration
        .as_ref()
        .is_some_and(|c| c.phase != Phase::Done);
    let _ = write!(s, ",\"calibrating\":{calibrating}");
    let _ = write!(s, ",\"calibrated\":{}", st.calibrated);
    s.push_str(",\"notice\":");
    match &st.notice {
        Some(n) => push_str(&mut s, n),
        None => s.push_str("null"),
    }
    if let Some(m) = extras.meters {
        s.push_str(",\"meters\":{\"in_vu\":");
        push_num(&mut s, m.in_vu_db);
        s.push_str(",\"out_vu\":");
        push_num(&mut s, m.out_vu_db);
        s.push_str(",\"in_level\":");
        push_num(&mut s, m.in_level_db);
        s.push_str(",\"out_level\":");
        push_num(&mut s, m.out_level_db);
        s.push_str(",\"gr\":");
        push_num(&mut s, m.gr_db);
        s.push_str(",\"lufs\":");
        match m.lufs {
            Some(l) => push_num(&mut s, l),
            None => s.push_str("null"),
        }
        let _ = write!(s, ",\"peak\":{}}}", m.peak);
    }
    if extras.presets {
        s.push_str(",\"presets\":[");
        for (i, p) in PresetId::ALL.into_iter().enumerate() {
            let b = builtin(p);
            if i > 0 {
                s.push(',');
            }
            let _ = write!(s, "{{\"id\":\"{}\",\"label\":", preset_id(p));
            push_str(&mut s, b.label);
            s.push_str(",\"description\":");
            push_str(&mut s, b.description);
            s.push('}');
        }
        s.push(']');
    }
    if let Some(dir) = extras.fonts_dir {
        s.push_str(",\"fonts_dir\":");
        push_str(&mut s, dir);
    }
    s.push('}');
    s
}

/// `{"error":"..."}` for a command that could not be applied.
pub fn error_json(message: &str) -> String {
    let mut s = String::from("{\"error\":");
    push_str(&mut s, message);
    s.push('}');
    s
}

/// A finite number with one decimal; JSON has no infinity, so silence
/// (`-inf` dB) and other non-finite values become `null`.
fn push_num(s: &mut String, v: f32) {
    if v.is_finite() {
        let _ = write!(s, "{v:.1}");
    } else {
        s.push_str("null");
    }
}

fn push_str(s: &mut String, v: &str) {
    s.push('"');
    for c in v.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(s, "\\u{:04x}", c as u32);
            }
            c => s.push(c),
        }
    }
    s.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Controller;

    fn state() -> UiState {
        let (handle, _processor) = engine::new_engine();
        let mut c = Controller::new(handle);
        c.set_neural_denoise(None);
        c.state().clone()
    }

    #[test]
    fn apply_changes_the_controller() {
        let (handle, _processor) = engine::new_engine();
        let mut c = Controller::new(handle);
        c.state_mut().model = ModelState::Ready;
        assert_eq!(apply(&mut c, Command::ToggleOn), Ok(Effect::None));
        assert!(!c.state().on);
        apply(&mut c, Command::SetOn(true)).unwrap();
        assert!(c.state().on);
        apply(&mut c, Command::ToggleMute).unwrap();
        assert!(c.state().muted);
        apply(&mut c, Command::SetMuted(false)).unwrap();
        assert!(!c.state().muted);
        apply(&mut c, Command::SetPreset(PresetId::Call)).unwrap();
        assert_eq!(c.state().preset, PresetId::Call);
        apply(&mut c, Command::SetNoise(Some(DenoiseLevel::Light))).unwrap();
        assert_eq!(c.state().neural_denoise, Some(DenoiseLevel::Light));
        apply(&mut c, Command::SetIntensity(25)).unwrap();
        assert_eq!(c.state().intensity, 25);
        apply(&mut c, Command::TogglePhones).unwrap();
        assert!(c.state().monitor.enabled);
        apply(&mut c, Command::SetPhones(false)).unwrap();
        assert!(!c.state().monitor.enabled);
        assert_eq!(apply(&mut c, Command::Show), Ok(Effect::Show));
        assert_eq!(apply(&mut c, Command::Quit), Ok(Effect::Quit));
    }

    #[test]
    fn noise_needs_the_model() {
        let (handle, _processor) = engine::new_engine();
        let mut c = Controller::new(handle);
        c.set_neural_denoise(None);
        c.state_mut().model = ModelState::Missing;
        let err = apply(&mut c, Command::SetNoise(Some(DenoiseLevel::Light))).unwrap_err();
        assert!(err.contains("ctl model download"), "{err}");
        assert_eq!(c.state().neural_denoise, None);
        assert_eq!(apply(&mut c, Command::SetNoise(None)), Ok(Effect::None));
        // A preset with noise removal applies without it and asks for the
        // model.
        apply(&mut c, Command::SetPreset(PresetId::Call)).unwrap();
        assert_eq!(c.state().preset, PresetId::Call);
        assert_eq!(c.state().neural_denoise, None);
        assert!(matches!(
            c.state().model_prompt,
            Some(crate::ModelUse::Noise(_))
        ));
        assert!(state_json(c.state(), Extras::default()).contains("\"model\":\"missing\""));
    }

    #[test]
    fn parses_every_command() {
        use Command as C;
        let cases = [
            ("status", C::Status),
            ("watch", C::Watch),
            ("on", C::SetOn(true)),
            ("off", C::SetOn(false)),
            ("toggle", C::ToggleOn),
            ("mute on", C::SetMuted(true)),
            ("mute toggle", C::ToggleMute),
            ("preset call", C::SetPreset(PresetId::Call)),
            ("noise off", C::SetNoise(None)),
            ("noise strong", C::SetNoise(Some(DenoiseLevel::Strong))),
            ("intensity 0", C::SetIntensity(0)),
            ("  intensity 100\n", C::SetIntensity(100)),
            ("phones on", C::SetPhones(true)),
            ("phones toggle", C::TogglePhones),
            ("model download", C::DownloadModel),
            ("show", C::Show),
            ("start", C::Start),
            ("quit", C::Quit),
        ];
        for (line, want) in cases {
            assert_eq!(Command::parse(line), Ok(want), "{line:?}");
        }
    }

    #[test]
    fn rejects_bad_commands() {
        for line in [
            "",
            "dance",
            "preset",
            "preset jazz",
            "noise loud",
            "intensity 101",
            "intensity -1",
            "intensity x",
            "on now",
            "phones",
            "mute",
            "mute loud",
            "model",
            "model delete",
            "preset call podcast",
        ] {
            assert!(Command::parse(line).is_err(), "{line:?}");
        }
    }

    #[test]
    fn ids_round_trip() {
        for p in PresetId::ALL {
            assert_eq!(preset_from_id(preset_id(p)), Some(p));
        }
        for l in [None, Some(DenoiseLevel::Light), Some(DenoiseLevel::Medium)] {
            assert_eq!(noise_from_id(noise_id(l)), Some(l));
        }
        assert_eq!(noise_from_id("strong"), Some(Some(DenoiseLevel::Strong)));
    }

    #[test]
    fn state_has_the_fields_a_bar_reads() {
        let json = state_json(&state(), Extras::default());
        for key in [
            "\"running\":true",
            "\"on\":true",
            "\"muted\":false",
            "\"preset\":\"broadcast\"",
            "\"preset_label\":\"Broadcast\"",
            "\"noise\":\"off\"",
            "\"intensity\":60",
            "\"phones\":false",
            "\"notice\":null",
        ] {
            assert!(json.contains(key), "{key} missing in {json}");
        }
        assert!(!json.contains("meters") && !json.contains("presets"));
        assert!(json.starts_with('{') && json.ends_with('}'));
    }

    #[test]
    fn calibrating_only_while_it_runs() {
        use crate::CalibrationStatus;
        let mut st = state();
        let status = |phase| CalibrationStatus {
            phase,
            seconds_left: 3.0,
            message: None,
            error: false,
        };
        st.calibration = Some(status(Phase::Speech));
        assert!(state_json(&st, Extras::default()).contains("\"calibrating\":true"));
        st.calibration = Some(status(Phase::Done));
        assert!(state_json(&st, Extras::default()).contains("\"calibrating\":false"));
    }

    #[test]
    fn extras_and_escaping() {
        let mut st = state();
        st.input_device = "Mic \"USB\"\\1\n".into();
        let meters = DisplayMeters {
            in_vu_db: -3.04,
            out_vu_db: f32::NEG_INFINITY,
            in_level_db: -2.0,
            out_level_db: f32::NEG_INFINITY,
            gr_db: 4.0,
            lufs: None,
            peak: true,
        };
        let json = state_json(
            &st,
            Extras {
                meters: Some(&meters),
                presets: true,
                fonts_dir: Some("/usr/share/voice-strip/fonts"),
            },
        );
        assert!(json.contains(r#""input":"Mic \"USB\"\\1\n""#), "{json}");
        assert!(
            json.contains(
                r#""meters":{"in_vu":-3.0,"out_vu":null,"in_level":-2.0,"out_level":null,"gr":4.0,"lufs":null,"peak":true}"#
            )
        );
        assert!(json.contains(r#"{"id":"narration","label":"Narration","#));
        assert!(json.contains(r#""fonts_dir":"/usr/share/voice-strip/fonts""#));
        assert_eq!(error_json("a \"b\""), r#"{"error":"a \"b\""}"#);
    }
}
