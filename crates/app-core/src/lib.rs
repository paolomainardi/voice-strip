//! Application logic shared by every frontend (GTK on Linux, native on
//! macOS): presets, calibration, voice profiles, and the [`Controller`]
//! that turns user intent into engine settings. No UI toolkit code here.

pub mod calibration;
pub mod controller;
pub mod presets;
pub mod remote;
pub mod tuning;

pub use controller::{CalibrationStatus, Controller, ModelState, ModelUse, UiState};
pub use denoise_neural::model;
pub use presets::{DenoiseLevel, Preset, PresetId};
pub use tuning::{CharacterLevels, VoiceProfile, character_levels, settings_for};
