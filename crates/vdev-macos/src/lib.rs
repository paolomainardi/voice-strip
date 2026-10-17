//! macOS side of the virtual microphone, in the app's process: captures the
//! microphone through the Core Audio HAL, runs the engine and feeds
//! `VoiceStrip.driver`.
//!
//! - [`convert`]: the adaptive resampler from the microphone's clock to the
//!   device timeline, pure Rust.
//! - [`monitor`]: the headphone monitor's queue and resampler to the
//!   headphones' clock, pure Rust.
//! - [`probe`]: a position-coded test signal and the check of its capture,
//!   pure Rust.
//! - `backend` (macOS only): [`engine::AudioBackend`] for the microphone and
//!   the virtual device.
//! - `hal` (macOS only): device lookup and capture through the client HAL API.
//!
//! Capturing any input device needs the microphone permission. macOS never
//! grants it to a process started over SSH and hands it silence instead, so
//! captures run in the desktop session (`scripts/macos-gui-run.sh`).

pub mod convert;
pub mod monitor;
pub mod probe;

#[cfg(target_os = "macos")]
pub mod backend;
#[cfg(target_os = "macos")]
pub mod hal;

#[cfg(target_os = "macos")]
pub use backend::MacBackend;
