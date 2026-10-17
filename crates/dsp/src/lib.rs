//! Voice processing chain. Pure DSP: no I/O, no threads, no allocation after
//! construction. Mono, 48 kHz, f32.

pub mod chain;
pub mod loudness;
pub mod settings;
pub mod stages;
pub mod util;

pub use chain::{Chain, ChainMeters, PathKind};
pub use settings::{ChainSettings, Module, ModuleSettings};
pub use stages::denoise::FrameDenoiser;

pub const SAMPLE_RATE: u32 = 48_000;
/// Main-path block: 10 ms.
pub const BLOCK_MAIN: usize = 480;
/// Monitor-path block.
pub const BLOCK_MONITOR: usize = 64;

/// A processing stage. `process` runs on the audio thread and must not
/// allocate, lock, or perform I/O.
pub trait Stage {
    /// Latency added by this stage, in samples at [`SAMPLE_RATE`].
    fn latency_samples(&self) -> usize;
    fn reset(&mut self);
    /// Process `buf` in place.
    fn process(&mut self, buf: &mut [f32]);
    /// Current gain reduction in dB (positive number), for metering.
    fn gain_reduction_db(&self) -> f32 {
        0.0
    }
}
