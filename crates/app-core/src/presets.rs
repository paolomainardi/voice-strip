#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PresetId {
    Broadcast,
    Podcast,
    Call,
    Stream,
    Narration,
}

impl PresetId {
    pub const ALL: [PresetId; 5] = [
        PresetId::Broadcast,
        PresetId::Podcast,
        PresetId::Call,
        PresetId::Stream,
        PresetId::Narration,
    ];
}

/// Neural noise removal strength. `None` in the state means the neural
/// denoiser is off and its 20 ms of latency is removed from the chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DenoiseLevel {
    Light,
    Medium,
    Strong,
}

impl DenoiseLevel {
    pub const ALL: [DenoiseLevel; 3] = [
        DenoiseLevel::Light,
        DenoiseLevel::Medium,
        DenoiseLevel::Strong,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DenoiseLevel::Light => "Light",
            DenoiseLevel::Medium => "Medium",
            DenoiseLevel::Strong => "Strong",
        }
    }

    /// Maximum noise attenuation, dB.
    pub fn attenuation_db(self) -> f32 {
        match self {
            DenoiseLevel::Light => 12.0,
            DenoiseLevel::Medium => 24.0,
            DenoiseLevel::Strong => 40.0,
        }
    }
}

/// A preset as shown in the UI. The full DSP definition (target curve,
/// dynamics) is loaded from versioned configuration files.
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub id: PresetId,
    /// Short label for the push button.
    pub label: &'static str,
    /// One-line description of the character.
    pub description: &'static str,
    pub target_lufs: f32,
    /// Neural noise removal when the preset is selected.
    pub default_denoise: Option<DenoiseLevel>,
}

pub fn builtin(id: PresetId) -> Preset {
    let (label, description, target_lufs) = match id {
        PresetId::Broadcast => (
            "Broadcast",
            "Warm, full radio sound, firm compression",
            -16.0,
        ),
        PresetId::Podcast => (
            "Podcast",
            "Natural and consistent, ready for -16 LUFS",
            -16.0,
        ),
        PresetId::Call => ("Call", "Maximum intelligibility, minimum noise", -18.0),
        PresetId::Stream => (
            "Stream",
            "Present and energetic, sits on top of music and games",
            -14.0,
        ),
        PresetId::Narration => (
            "Narration",
            "Intimate and dynamic, minimal intervention",
            -18.0,
        ),
    };
    let default_denoise = match id {
        PresetId::Call => Some(DenoiseLevel::Strong),
        PresetId::Stream => Some(DenoiseLevel::Medium),
        PresetId::Broadcast | PresetId::Podcast | PresetId::Narration => None,
    };
    Preset {
        id,
        label,
        description,
        target_lufs,
        default_denoise,
    }
}
