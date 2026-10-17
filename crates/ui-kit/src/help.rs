//! Tooltip texts shared by both views: what each control does and what it
//! changes, in plain words.

use app_core::presets::builtin;
use app_core::{DenoiseLevel, ModelState, PresetId, UiState};
use dsp::Module;

pub fn on_switch(state: &UiState) -> String {
    if state.on {
        "Processing is on. Click to bypass: your microphone goes out unprocessed.".into()
    } else {
        "Processing is bypassed. Click to turn the voice chain back on.".into()
    }
}

pub fn mute(state: &UiState) -> String {
    if state.muted {
        "Muted: others hear silence. Click (or press M) to unmute.".into()
    } else {
        "Mute: others hear silence until you unmute; your input meter keeps moving so you \
         can see the mic is live. Shortcut: M."
            .into()
    }
}

pub fn bypass(state: &UiState) -> String {
    if state.on {
        "Bypass: send the original microphone, with the same delay so nothing jumps.".into()
    } else {
        "Bypassed. Click to turn processing back on.".into()
    }
}

pub fn preset(p: PresetId) -> String {
    let b = builtin(p);
    let noise = match b.default_denoise {
        Some(l) => format!("noise removal {}", l.label().to_lowercase()),
        None => "noise removal off".into(),
    };
    format!(
        "{}: {}. Loudness target {:.0} LUFS, {noise}.",
        b.label, b.description, b.target_lufs
    )
}

/// Noise removal button. Without the model, a level also says that it
/// needs the one-time download.
pub fn noise(level: Option<DenoiseLevel>, state: &UiState) -> String {
    let mut s: String = match level {
        None => {
            "Noise removal off: lowest latency. Pauses are still kept quiet by the expander.".into()
        }
        Some(l) => format!(
            "Noise removal {}: removes up to {:.0} dB of background noise (fans, keyboard, \
             room). Adds 20 ms of delay to the microphone.",
            l.label().to_lowercase(),
            l.attenuation_db()
        ),
    };
    if level.is_some() && state.model != ModelState::Ready {
        s.push_str(" Needs a one-time 34.7 MB model download.");
    }
    s
}

pub fn intensity(state: &UiState) -> String {
    format!(
        "Intensity {}: how strongly the preset shapes your voice (EQ, compression, \
         saturation). Drag, scroll or use the arrow keys.",
        state.intensity
    )
}

pub fn ab(state: &UiState) -> String {
    if state.ab_original {
        "B: you hear your original microphone. Click to go back to the processed sound (A).".into()
    } else {
        "A/B: compare the processed sound (A) with your original microphone (B).".into()
    }
}

pub fn monitor(state: &UiState) -> String {
    format!(
        "Headphone monitoring: hear yourself in headphones ({:.1} ms). Use headphones, not \
         speakers, to avoid feedback. The headphones get a lighter chain than the virtual \
         microphone, without noise removal, saturation or leveler, to keep the delay low. \
         Bluetooth headphones add 150 ms or more of their own: use wired ones to monitor.",
        state.monitor_latency_ms
    )
}

pub fn monitor_level() -> String {
    "Headphone level: how loud you hear yourself. It does not change what others hear.".into()
}

pub fn monitor_denoise() -> String {
    "Noise removal in headphones: cleaner, but adds 20 ms, which can make your own voice \
     sound delayed."
        .into()
}

pub fn calibrate() -> String {
    "Calibrate (15 s): stay quiet for 5 s, then speak normally for 10 s. Sets your input \
     level, noise floor and de-esser frequency for this microphone."
        .into()
}

pub fn advanced() -> String {
    "Open the advanced view: spectrum, loudness and every processing stage.".into()
}

pub fn compact() -> String {
    "Back to the compact view.".into()
}

pub fn vu_input() -> String {
    "Input level (VU) before processing. The lamp lights when the microphone gets close to \
     clipping: lower its gain."
        .into()
}

pub fn vu_output(state: &UiState) -> String {
    format!(
        "Output level (VU) after processing, and its short-term loudness in LUFS. The preset \
         aims at {:.0} LUFS.",
        builtin(state.preset).target_lufs
    )
}

pub fn gain_reduction() -> String {
    "Gain reduction: how much the compressor and limiter are turning loud moments down, in \
     dB. A few LEDs while you speak is normal."
        .into()
}

pub fn module(m: Module, state: &UiState) -> String {
    let what = match m {
        Module::Input => {
            "Input: removes low rumble and boominess, and applies the input gain set by calibration."
        }
        Module::Denoise => {
            "Denoise: neural noise removal (when on) plus an expander that keeps pauses quiet."
        }
        Module::DePlosive => "De-plosive: tames the low thumps of 'p' and 'b' sounds.",
        Module::DeEsser => {
            "De-esser: softens harsh 's' and 'sh' sounds, at the frequency found during calibration."
        }
        Module::DynamicEq => {
            "Dynamic EQ: shapes your voice toward the preset's sound and cuts mud, nasality or harshness only when they stick out."
        }
        Module::Compressor => "Compressor: evens out loud and soft words.",
        Module::Saturation => "Saturation: adds subtle warmth and density.",
        Module::Leveler => "Leveler: slowly rides your level toward the preset's loudness target.",
        Module::Limiter => "Limiter: catches peaks so the output never goes above -1 dBTP.",
    };
    let state_text = if state.module_enabled[m.index()] {
        "Click the power button to switch it off."
    } else {
        "Switched off. Click the power button to switch it on."
    };
    let phones = if m.in_monitor_path() {
        " Also active in headphone monitoring (M)."
    } else {
        ""
    };
    format!("{what}{phones} {state_text}")
}

pub fn device_input(state: &UiState) -> String {
    let name = if state.input_device.is_empty() {
        "Default microphone"
    } else {
        &state.input_device
    };
    format!("Microphone: {name}. Click to choose another.")
}

pub fn device_output(state: &UiState) -> String {
    let name = if state.output_device.is_empty() {
        "Default output"
    } else {
        &state.output_device
    };
    format!("Headphones: {name}. Click to choose another.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_module_and_preset_has_help() {
        let st = crate::demo::demo_state();
        for m in Module::ALL {
            assert!(module(m, &st).len() > 30, "{m:?}");
        }
        for p in PresetId::ALL {
            assert!(preset(p).contains("LUFS"), "{p:?}");
        }
        assert!(noise(Some(DenoiseLevel::Strong), &crate::demo::demo_state()).contains("40 dB"));
    }
}
