use std::fmt;

use crate::processor::AudioProcessor;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Stable identifier used to select the device (node name on PipeWire).
    pub id: String,
    /// Human-readable name.
    pub label: String,
}

#[derive(Debug)]
pub struct BackendError(pub String);

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BackendError {}

/// Platform audio I/O. A backend captures the selected physical input,
/// calls [`AudioProcessor`] on its realtime thread, writes the main output
/// to the virtual microphone and, when enabled, the monitor output to the
/// selected headphones.
pub trait AudioBackend {
    fn list_inputs(&self) -> Result<Vec<DeviceInfo>, BackendError>;
    fn list_outputs(&self) -> Result<Vec<DeviceInfo>, BackendError>;

    /// Starts audio. `input`/`monitor_output` of `None` mean the system default.
    fn start(
        &mut self,
        processor: AudioProcessor,
        input: Option<&str>,
        monitor_output: Option<&str>,
    ) -> Result<Box<dyn RunningBackend>, BackendError>;
}

/// A started backend. Dropping it stops audio and removes the virtual device.
pub trait RunningBackend: Send {
    /// Name of the virtual microphone as other applications see it.
    fn virtual_device_name(&self) -> &str;
    /// Measured or reported device latency added by the backend itself
    /// (buffers, quantum), in samples, for the main and monitor paths.
    fn io_latency_samples(&self) -> (usize, usize);
    /// Switches the physical microphone while running (`None`: system
    /// default). Backends that cannot switch ignore it.
    fn set_input(&self, _input: Option<&str>) {}
    /// Switches the headphones used by the monitor path (`None`: system
    /// default). Backends that cannot switch ignore it.
    fn set_monitor_output(&self, _output: Option<&str>) {}
    /// Whether some application reads the virtual microphone now; `None`
    /// when the backend cannot tell (capture then never pauses).
    fn has_readers(&self) -> Option<bool> {
        None
    }
    /// Pauses (`false`) or resumes capture of the physical microphone.
    /// Paused, the microphone is released, so its in-use indicator goes
    /// off, and nothing is processed. Returns false when it could not be
    /// done (the caller retries).
    fn set_capture(&self, _on: bool) -> bool {
        true
    }
}
