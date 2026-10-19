//! Deterministic state for screenshots and tests: the numbers shown in the
//! design sources.

use app_core::{DenoiseLevel, DetectorState, ModelState, PresetId, UiState};
use dsp::ChainSettings;
use engine::{MeterFrame, MonitorConfig};

pub fn demo_state() -> UiState {
    let mut chain = ChainSettings::default();
    chain.modules.input_trim_db = 6.0;
    chain.modules.denoise_mix = 0.7;
    chain.modules.deplosive_threshold_db = -24.0;
    chain.modules.deesser_freq_hz = 6800.0;
    chain.modules.comp_ratio = 4.0;
    chain.modules.saturation_drive = 0.15;
    chain.modules.leveler_target_lufs = Some(-16.0);
    chain.modules.limiter_ceiling_dbtp = -1.0;
    chain.modules.eq_dynamic_threshold_db = [-20.0, -20.0, -20.0, 0.0];
    chain.modules.eq_static_db = [-3.0, -1.0, 2.0, 1.0];
    UiState {
        on: true,
        muted: false,
        ab_original: false,
        preset: PresetId::Broadcast,
        intensity: 60,
        neural_denoise: Some(DenoiseLevel::Medium),
        monitor: MonitorConfig {
            enabled: false,
            with_denoise: false,
            level: 0.7,
        },
        module_enabled: [true; 9],
        meters: demo_meters(),
        mic_latency_ms: 21.2,
        monitor_latency_ms: 4.2,
        output_device: String::new(),
        input_device: "Physical microphone".into(),
        virtual_device: "Voice Strip".into(),
        chain,
        voice_profile: "Default".into(),
        calibrated: true,
        cpu_percent: 3.1,
        calibration: None,
        notice: None,
        model: ModelState::Ready,
        model_prompt: None,
        voice_detector: DetectorState::Running,
        update_available: None,
        update_check: true,
    }
}

/// Meter values matching the design mock-up (VU reference -18 dBFS).
pub fn demo_meters() -> MeterFrame {
    MeterFrame {
        in_peak_db: -9.8,
        in_rms_db: -24.0,
        out_rms_db: -19.84,
        in_avg_db: -24.0,
        out_avg_db: -19.84,
        lufs_momentary: -15.4,
        lufs_short_term: -16.2,
        true_peak_db: -1.2,
        gain_reduction_db: [0.0, 18.0, 2.1, 3.4, 2.6, 4.5, 0.4, -1.2, 0.8],
        denoise_missed_frames: 0,
        speech_probability: Some(0.94),
        room_db: Some(-52.0),
        vad_inference_ms: Some(0.04),
        denoise_inference_ms: Some(0.13),
    }
}
