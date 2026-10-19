//! PipeWire backend: captures the physical microphone and exposes the
//! processed signal as a virtual source other applications can select.
//!
//! # Topology
//!
//! Two nodes owned by one client connection:
//!
//! - `voice_strip_dsp`, a `pw_filter` with DSP ports (mono F32 at the graph
//!   rate): an input port that this backend links itself, with
//!   `link-factory`, to the first mono/left output port of the selected
//!   source node (default source from the session metadata when none is
//!   given); an output port; and, only while headphone monitoring is on, a
//!   second output port `monitor` linked to every input port of the
//!   selected sink. A disabled monitor costs nothing.
//! - `voice_strip_mic`, the virtual microphone other applications select: a
//!   `support.null-audio-sink` adapter (media.class `Audio/Source/Virtual`)
//!   whose input port is linked to the filter output. A plain `pw_filter`
//!   node is not listed by pipewire-pulse, which needs a node with format
//!   and property params; the adapter provides them. `priority.driver` is 0
//!   so the physical source's driver stays the graph driver.
//!
//! Capture, processing and the virtual source are linked nodes under one
//! driver, so the scheduler runs them in one graph cycle: the realtime
//! callback reads the capture buffer and writes the output buffer, and the
//! adapter forwards it in the same cycle. The loopback test measures that
//! the only latency between the physical source and the virtual microphone
//! is the DSP latency. A two-stream design
//! (capture stream and playback stream joined by a ring buffer) would add at
//! least one cycle and jitter between two nodes whose relative order is not
//! defined.
//!
//! The nodes request the graph quantum through `node.latency`
//! ([`PipeWireBackend::with_quantum`], default 480/48000 so a solo graph runs
//! at the neural frame size with the shortest handoff, which PipeWire rounds
//! down to 256 when `clock.power-of-two-quantum` is on; 64/48000 while the
//! monitor is on). Any quantum works: the callback declares the current
//! block size to the engine, which sizes the neural handoff from it.
//!
//! # Threads
//!
//! The PipeWire main loop runs on its own thread, started by
//! [`PipeWireBackend::start`]. It is controlled through a
//! `pipewire::channel`; nothing blocks the caller. The process callback
//! runs on PipeWire's realtime data loop (`PW_FILTER_FLAG_RT_PROCESS`), which
//! the server schedules with realtime priority; no extra thread is spawned.
//! Only atomics and the engine's lock-free queues are shared with it.

#![cfg(target_os = "linux")]

mod filter;
#[doc(hidden)]
pub mod graph;
pub mod params;

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use engine::{AudioBackend, AudioProcessor, BackendError, DeviceInfo, RunningBackend};
use pipewire as pw;

pub use filter::DSP_NODE_NAME;
pub use graph::list_devices;

/// Name of the virtual microphone node.
pub const VIRTUAL_MIC_NAME: &str = "voice_strip_mic";
/// Description shown by applications.
pub const VIRTUAL_MIC_DESCRIPTION: &str = "Voice Strip Microphone";
/// Quantum requested while the headphone monitor is on.
pub const MONITOR_QUANTUM: u32 = 64;

/// Factory for [`PipeWireRunning`].
#[derive(Clone, Debug)]
pub struct PipeWireBackend {
    quantum: u32,
}

impl Default for PipeWireBackend {
    fn default() -> Self {
        Self {
            quantum: dsp::BLOCK_MAIN as u32,
        }
    }
}

impl PipeWireBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Quantum (samples at 48 kHz) requested from the graph through
    /// `node.latency` while the monitor is off.
    pub fn with_quantum(mut self, quantum: u32) -> Self {
        self.quantum = quantum.max(1);
        self
    }

    /// Starts the backend and returns the concrete running handle.
    pub fn start_pipewire(
        &self,
        processor: AudioProcessor,
        input: Option<&str>,
        monitor_output: Option<&str>,
    ) -> Result<PipeWireRunning, BackendError> {
        let (tx, rx) = pw::channel::channel::<Command>();
        let status = Arc::new(Status::default());
        let cfg = Config {
            quantum: self.quantum,
            input: input.map(str::to_owned),
            monitor_output: monitor_output.map(str::to_owned),
        };
        let status_t = status.clone();
        let thread = std::thread::Builder::new()
            .name("voice-strip-pw".into())
            .spawn(move || {
                if let Err(e) = filter::run(cfg, processor, rx, status_t.clone()) {
                    status_t.set_error(e.0);
                }
                status_t.stopped.store(true, Ordering::Release);
            })
            .map_err(|e| BackendError(e.to_string()))?;
        let running = PipeWireRunning {
            tx,
            thread: Some(thread),
            status,
        };
        // Surface connection errors early.
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Some(e) = running.status.error() {
                return Err(BackendError(e));
            }
            if running.status.connected.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(running)
    }
}

impl AudioBackend for PipeWireBackend {
    fn list_inputs(&self) -> Result<Vec<DeviceInfo>, BackendError> {
        list_devices(true)
    }

    fn list_outputs(&self) -> Result<Vec<DeviceInfo>, BackendError> {
        list_devices(false)
    }

    fn start(
        &mut self,
        processor: AudioProcessor,
        input: Option<&str>,
        monitor_output: Option<&str>,
    ) -> Result<Box<dyn RunningBackend>, BackendError> {
        Ok(Box::new(self.start_pipewire(
            processor,
            input,
            monitor_output,
        )?))
    }
}

pub(crate) struct Config {
    pub quantum: u32,
    pub input: Option<String>,
    pub monitor_output: Option<String>,
}

pub(crate) enum Command {
    Stop,
    /// Capture from this source node (`None`: follow the default source).
    SetInput(Option<String>),
    /// Send the monitor to this sink node (`None`: follow the default sink).
    SetMonitorOutput(Option<String>),
    /// Link (`true`) or unlink the microphone.
    SetCapture(bool),
}

/// State published by the PipeWire thread and the realtime callback.
#[derive(Default)]
pub(crate) struct Status {
    pub connected: AtomicBool,
    pub stopped: AtomicBool,
    /// Capture link to the source established.
    pub linked: AtomicBool,
    /// Filter output linked to the virtual microphone node.
    pub output_linked: AtomicBool,
    /// Monitor port linked to the sink.
    pub monitor_linked: AtomicBool,
    /// Block size seen by the last realtime callback.
    pub quantum: AtomicUsize,
    /// Driver delay to the hardware reported by the graph clock, samples.
    pub driver_delay: AtomicI64,
    /// Realtime callbacks run so far.
    pub cycles: AtomicUsize,
    /// Links reading the virtual microphone (other applications).
    pub readers: AtomicUsize,
    error: Mutex<Option<String>>,
}

impl Status {
    fn set_error(&self, e: String) {
        if let Ok(mut g) = self.error.lock()
            && g.is_none()
        {
            *g = Some(e);
        }
    }

    fn error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|g| g.clone())
    }
}

/// A started PipeWire backend. Dropping it stops the loop thread and
/// removes the virtual microphone.
pub struct PipeWireRunning {
    tx: pw::channel::Sender<Command>,
    thread: Option<JoinHandle<()>>,
    status: Arc<Status>,
}

impl PipeWireRunning {
    /// Whether the source is linked to the filter and the filter to the
    /// virtual microphone.
    pub fn is_linked(&self) -> bool {
        self.status.linked.load(Ordering::Acquire)
            && self.status.output_linked.load(Ordering::Acquire)
    }

    /// Whether the monitor port is linked to the sink.
    pub fn is_monitor_linked(&self) -> bool {
        self.status.monitor_linked.load(Ordering::Acquire)
    }

    /// Block size of the last realtime callback (0 before the first).
    pub fn quantum(&self) -> usize {
        self.status.quantum.load(Ordering::Acquire)
    }

    /// Realtime callbacks run so far.
    pub fn cycles(&self) -> usize {
        self.status.cycles.load(Ordering::Relaxed)
    }

    pub fn error(&self) -> Option<String> {
        self.status.error()
    }

    /// Blocks until the capture link is up, the monitor link too when
    /// `monitor` is set, or the timeout elapses.
    pub fn wait_linked(&self, monitor: bool, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.is_linked() && (!monitor || self.is_monitor_linked()) && self.cycles() > 2 {
                return true;
            }
            if self.status.stopped.load(Ordering::Acquire) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }
}

impl RunningBackend for PipeWireRunning {
    fn virtual_device_name(&self) -> &str {
        VIRTUAL_MIC_NAME
    }

    fn io_latency_samples(&self) -> (usize, usize) {
        // Capture and virtual source live in one node, so no graph cycle is
        // added. What remains is the driver's delay to its hardware.
        let d = self.status.driver_delay.load(Ordering::Relaxed).max(0) as usize;
        (d, d)
    }

    fn set_input(&self, input: Option<&str>) {
        let _ = self.tx.send(Command::SetInput(input.map(str::to_owned)));
    }

    fn set_monitor_output(&self, output: Option<&str>) {
        let _ = self
            .tx
            .send(Command::SetMonitorOutput(output.map(str::to_owned)));
    }

    fn has_readers(&self) -> Option<bool> {
        self.status
            .connected
            .load(Ordering::Acquire)
            .then(|| self.status.readers.load(Ordering::Acquire) > 0)
    }

    fn set_capture(&self, on: bool) {
        let _ = self.tx.send(Command::SetCapture(on));
    }
}

impl Drop for PipeWireRunning {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
