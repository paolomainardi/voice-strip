//! Maps a preset, the Intensity knob and the calibrated voice profile to
//! concrete chain settings.

use dsp::ModuleSettings;

use crate::presets::{DenoiseLevel, PresetId, builtin};

/// Result of calibration for one person and microphone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceProfile {
    /// Gain bringing the speaker's speech to [`CALIBRATED_LUFS`].
    pub trim_db: f32,
    /// Noise floor measured in silence, dBFS (after trim).
    pub noise_floor_db: f32,
    /// Detected sibilance center frequency.
    pub sibilance_hz: f32,
    /// Static correction toward the target voice curve, per EQ band.
    pub eq_correction_db: [f32; 4],
}

/// Speech level after the input trim of a calibrated profile.
pub const CALIBRATED_LUFS: f32 = -23.0;

impl Default for VoiceProfile {
    fn default() -> Self {
        Self {
            trim_db: 0.0,
            noise_floor_db: -70.0,
            sibilance_hz: 6500.0,
            eq_correction_db: [0.0; 4],
        }
    }
}

/// Per-preset character at full intensity.
struct Character {
    hpf_hz: f32,
    proximity_db: f32,
    /// Expander threshold above the noise floor, dB.
    expander_margin_db: f32,
    deesser_offset_db: f32,
    eq_db: [f32; 4],
    eq_dyn_db: [f32; 4],
    comp_ratio: f32,
    comp_below_db: f32,
    comp_mix: f32,
    saturation: f32,
    leveler_speed: f32,
}

fn character(p: PresetId) -> Character {
    match p {
        PresetId::Broadcast => Character {
            hpf_hz: 80.0,
            proximity_db: -1.0,
            expander_margin_db: 8.0,
            deesser_offset_db: -8.0,
            eq_db: [-1.5, -0.5, 3.5, 1.0],
            eq_dyn_db: [-8.0, -14.0, -16.0, 0.0],
            comp_ratio: 4.0,
            comp_below_db: 10.0,
            comp_mix: 0.85,
            saturation: 0.3,
            leveler_speed: 1.0,
        },
        PresetId::Podcast => Character {
            hpf_hz: 90.0,
            proximity_db: -2.0,
            expander_margin_db: 8.0,
            deesser_offset_db: -5.0,
            eq_db: [-1.0, 0.0, 2.0, 1.0],
            eq_dyn_db: [-8.0, -14.0, -16.0, 0.0],
            comp_ratio: 3.0,
            comp_below_db: 8.0,
            comp_mix: 0.8,
            saturation: 0.0,
            leveler_speed: 0.6,
        },
        PresetId::Call => Character {
            hpf_hz: 120.0,
            proximity_db: -3.0,
            expander_margin_db: 12.0,
            deesser_offset_db: -6.0,
            eq_db: [-3.0, -1.0, 4.0, 0.0],
            eq_dyn_db: [-6.0, -12.0, -16.0, 0.0],
            comp_ratio: 2.0,
            comp_below_db: 6.0,
            comp_mix: 1.0,
            saturation: 0.0,
            leveler_speed: 1.0,
        },
        PresetId::Stream => Character {
            hpf_hz: 90.0,
            proximity_db: -1.5,
            expander_margin_db: 8.0,
            deesser_offset_db: -8.0,
            eq_db: [-1.5, -0.5, 4.5, 1.5],
            eq_dyn_db: [-8.0, -14.0, -15.0, 0.0],
            comp_ratio: 5.0,
            comp_below_db: 12.0,
            comp_mix: 0.9,
            saturation: 0.45,
            leveler_speed: 1.0,
        },
        PresetId::Narration => Character {
            hpf_hz: 70.0,
            proximity_db: -1.0,
            expander_margin_db: 6.0,
            deesser_offset_db: -4.0,
            eq_db: [-1.0, 0.0, 1.5, 0.5],
            eq_dyn_db: [-9.0, -15.0, -17.0, 0.0],
            comp_ratio: 2.0,
            comp_below_db: 6.0,
            comp_mix: 0.6,
            saturation: 0.0,
            leveler_speed: 0.25,
        },
    }
}

/// Builds chain settings. `intensity` is 0..=100; 0 keeps only clean-up
/// (high-pass, de-plosive, leveler, limiter) and the profile correction.
pub fn settings_for(
    preset: PresetId,
    intensity: u8,
    denoise: Option<DenoiseLevel>,
    profile: &VoiceProfile,
) -> ModuleSettings {
    let c = character(preset);
    let k = intensity.min(100) as f32 / 100.0;
    let mut eq = [0.0; 4];
    for (i, v) in eq.iter_mut().enumerate() {
        *v = profile.eq_correction_db[i] + k * c.eq_db[i];
    }
    ModuleSettings {
        input_trim_db: profile.trim_db,
        input_invert_polarity: false,
        hpf_hz: c.hpf_hz,
        proximity_comp_db: k * c.proximity_db,
        denoise_mix: if denoise.is_some() { 1.0 } else { 0.0 },
        denoise_atten_limit_db: denoise.map_or(0.0, DenoiseLevel::attenuation_db),
        denoise_expander_threshold_db: profile.noise_floor_db + c.expander_margin_db * k,
        deplosive_threshold_db: CALIBRATED_LUFS - 6.0,
        deesser_freq_hz: profile.sibilance_hz,
        deesser_threshold_db: CALIBRATED_LUFS + 3.0 + c.deesser_offset_db * k,
        eq_static_db: eq,
        eq_dynamic_threshold_db: if k > 0.0 { c.eq_dyn_db } else { [0.0; 4] },
        comp_threshold_db: CALIBRATED_LUFS + 3.0 - c.comp_below_db * k,
        comp_ratio: 1.0 + (c.comp_ratio - 1.0) * k,
        comp_parallel_mix: c.comp_mix,
        saturation_drive: c.saturation * k,
        leveler_target_lufs: Some(builtin(preset).target_lufs),
        leveler_speed: c.leveler_speed,
        limiter_ceiling_dbtp: -1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_differ_and_carry_their_targets() {
        let p = VoiceProfile::default();
        let all: Vec<_> = PresetId::ALL
            .iter()
            .map(|&id| settings_for(id, 60, None, &p))
            .collect();
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                assert_ne!(all[i], all[j]);
            }
        }
        assert_eq!(all[0].leveler_target_lufs, Some(-16.0));
        assert_eq!(all[3].leveler_target_lufs, Some(-14.0));
    }

    #[test]
    fn zero_intensity_disables_shaping() {
        let s = settings_for(PresetId::Stream, 0, None, &VoiceProfile::default());
        assert_eq!(s.comp_ratio, 1.0);
        assert_eq!(s.saturation_drive, 0.0);
        assert_eq!(s.eq_static_db, [0.0; 4]);
    }

    #[test]
    fn narration_compresses_less_than_broadcast() {
        let p = VoiceProfile::default();
        let b = settings_for(PresetId::Broadcast, 100, None, &p);
        let n = settings_for(PresetId::Narration, 100, None, &p);
        assert!(n.comp_ratio < b.comp_ratio);
        assert!(n.comp_threshold_db > b.comp_threshold_db);
    }
}
