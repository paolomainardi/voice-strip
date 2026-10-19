//! Realtime engine, platform-neutral. Owns the processing chains and the
//! lock-free channels between the audio thread and the UI. Platform audio
//! I/O lives in backend crates (`vdev-linux`, `vdev-macos`) that
//! implement [`AudioBackend`] and drive an [`AudioProcessor`].

pub mod backend;
pub mod denormals;
pub mod meters;
pub mod processor;
pub mod sim;
pub mod standby;

pub use backend::{AudioBackend, BackendError, DeviceInfo, RunningBackend};
pub use meters::MeterFrame;
pub use processor::{
    AudioProcessor, EngineHandle, MAX_HOST_BLOCK, MonitorConfig, ProcessorWatch, new_engine,
    new_engine_with,
};
