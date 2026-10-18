/// The nine modules, in fixed processing order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Module {
    Input,
    Denoise,
    DePlosive,
    DeEsser,
    DynamicEq,
    Compressor,
    Saturation,
    Leveler,
    Limiter,
}

impl Module {
    pub const ALL: [Module; 9] = [
        Module::Input,
        Module::Denoise,
        Module::DePlosive,
        Module::DeEsser,
        Module::DynamicEq,
        Module::Compressor,
        Module::Saturation,
        Module::Leveler,
        Module::Limiter,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Whether the module runs in the headphone monitor path.
    pub fn in_monitor_path(self) -> bool {
        !matches!(self, Module::Denoise | Module::Saturation | Module::Leveler)
    }

    pub fn name(self) -> &'static str {
        match self {
            Module::Input => "Input",
            Module::Denoise => "Denoise",
            Module::DePlosive => "De-plosive",
            Module::DeEsser => "De-esser",
            Module::DynamicEq => "Dynamic EQ",
            Module::Compressor => "Compressor",
            Module::Saturation => "Saturation",
            Module::Leveler => "Leveler",
            Module::Limiter => "Limiter",
        }
    }
}

/// Per-module settings. Plain data, `Copy`, so it can cross the
/// UI-to-audio-thread queue without allocation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModuleSettings {
    pub input_trim_db: f32,
    pub input_invert_polarity: bool,
    /// High-pass corner, Hz. 0 disables it.
    pub hpf_hz: f32,
    /// Low-shelf cut at ~150 Hz compensating proximity effect, dB (<= 0).
    pub proximity_comp_db: f32,

    pub denoise_mix: f32,
    /// Maximum noise attenuation of the neural denoiser, dB. Lower values
    /// keep a natural room tone instead of digital silence.
    pub denoise_atten_limit_db: f32,
    pub denoise_expander_threshold_db: f32,
    /// Expander threshold above the room noise the voice detector measures,
    /// dB. When set and the room is known, it replaces
    /// `denoise_expander_threshold_db`. `None` keeps the fixed threshold.
    pub denoise_expander_margin_db: Option<f32>,

    pub deplosive_threshold_db: f32,

    pub deesser_freq_hz: f32,
    pub deesser_threshold_db: f32,

    /// Static shaping toward the target voice curve: gains in dB at the
    /// fixed `dynamic_eq` band centers (mud, nasal, presence, air).
    pub eq_static_db: [f32; 4],
    pub eq_dynamic_threshold_db: [f32; 4],

    pub comp_threshold_db: f32,
    pub comp_ratio: f32,
    pub comp_parallel_mix: f32,

    pub saturation_drive: f32,

    /// `None` disables the leveler (gain slews back to 0 dB).
    pub leveler_target_lufs: Option<f32>,
    /// How quickly the leveler follows level changes, 0..=1 (1 = fastest).
    /// Slower keeps more of the speaker's dynamics.
    pub leveler_speed: f32,

    pub limiter_ceiling_dbtp: f32,
}

impl Default for ModuleSettings {
    /// Neutral: every stage transparent except limiter ceiling.
    fn default() -> Self {
        Self {
            input_trim_db: 0.0,
            input_invert_polarity: false,
            hpf_hz: 0.0,
            proximity_comp_db: 0.0,
            denoise_mix: 0.0,
            denoise_atten_limit_db: 25.0,
            denoise_expander_threshold_db: -120.0,
            denoise_expander_margin_db: None,
            deplosive_threshold_db: 0.0,
            deesser_freq_hz: 6500.0,
            deesser_threshold_db: 0.0,
            eq_static_db: [0.0; 4],
            eq_dynamic_threshold_db: [0.0; 4],
            comp_threshold_db: 0.0,
            comp_ratio: 1.0,
            comp_parallel_mix: 1.0,
            saturation_drive: 0.0,
            leveler_target_lufs: None,
            leveler_speed: 1.0,
            limiter_ceiling_dbtp: -1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChainSettings {
    pub modules: ModuleSettings,
    /// Per-module enable, indexed by [`Module::index`].
    pub enabled: [bool; 9],
    /// Processed signal replaced by the dry input (A/B "original").
    pub bypass: bool,
    /// Output silenced (with a short fade), for the mute button.
    pub mute: bool,
}

impl Default for ChainSettings {
    fn default() -> Self {
        Self {
            modules: ModuleSettings::default(),
            enabled: [true; 9],
            bypass: false,
            mute: false,
        }
    }
}
