//! Voice Strip virtual microphone for macOS: an AudioServerPlugIn that
//! `coreaudiod` loads from `/Library/Audio/Plug-Ins/HAL/VoiceStrip.driver`
//! and that publishes one input device, `Voice Strip`.
//!
//! The app writes audio into a shared memory ring ([`shm`]) on the device
//! timeline ([`clock`]); the driver plays what the ring holds, and silence
//! where it holds nothing.
//!
//! - [`hal`]: Core Audio constants and plain-data types from the SDK.
//! - [`model`]: the object tree and every property value, pure Rust.
//! - [`clock`]: the device timeline and its zero time stamps, pure Rust.
//! - [`shm`]: the shared memory segment the app writes and the driver reads.
//! - `plugin` (macOS only): the C interface `coreaudiod` calls.
//!
//! A plug-in must not call the client HAL API (`AudioObjectGetPropertyData`
//! and friends) and runs sandboxed inside `coreaudiod`: it only reads its
//! bundle, and every Mach service it uses is listed in `Info.plist`.

pub mod clock;
pub mod hal;
pub mod model;
#[cfg(target_os = "macos")]
mod plugin;
pub mod shm;
