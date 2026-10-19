//! The macOS audio backend: captures the selected microphone with an
//! IOProc, converts it onto the virtual device's timeline, runs the engine
//! and writes the result into the shared ring `VoiceStrip.driver` plays.
//!
//! All realtime work happens in the microphone's IOProc: copy channel 0,
//! convert ([`Converter`]), `AudioProcessor::process`, ring write. No
//! allocation, lock or wait there; the neural model keeps its own worker.
//! Device queries, buffer sizing and the shared memory segment are set up
//! before the IOProc starts.
//!
//! Headphone monitor: while the UI wants it, a control thread runs a second
//! IOProc on the headphones. The microphone's IOProc passes the engine's
//! monitor output through a lock-free queue, and [`MonitorOut`] plays it on
//! the headphones' clock. With monitoring off there is no headphone IOProc
//! and the engine skips the monitor chain.

use std::collections::hash_map::RandomState;
use std::ffi::c_void;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::*};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use engine::{
    AudioBackend, AudioProcessor, BackendError, DeviceInfo, ProcessorWatch, RunningBackend,
};
use vdev_macos_driver::hal::{
    AudioObjectId, FORMAT_LINEAR_PCM, NO_ERROR, OsStatus, StreamBasicDescription, TimeStamp,
};
use vdev_macos_driver::{clock, model, shm};

use crate::convert::Converter;
use crate::hal::{self, AudioBufferList, IoProcId};
use crate::monitor::{MonitorOut, target_fill};

/// IO buffer the backend asks the microphone and the headphones for, frames.
/// 32 divides the neural hop (480), which keeps its handoff short.
const CAPTURE_FRAMES: u32 = 32;
/// Monitor queue capacity, frames (85 ms).
const MONITOR_QUEUE: usize = 4096;
/// kAudioTimeStampSampleTimeValid | kAudioTimeStampHostTimeValid.
const TIMES_VALID: u32 = 0x3;
const FLAG_FLOAT: u32 = 0x1;
const FLAG_NON_INTERLEAVED: u32 = 0x20;

/// Replaces the microphone's samples with a known signal while keeping its
/// real callbacks, block sizes and time stamps (tests only).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TestSignal {
    /// A 440 Hz sine at -12 dBFS from the source sample time.
    Sine,
    /// One full-scale sample every source second, at whole seconds of the
    /// source sample time; the timeline position of each is recorded.
    Clicks,
}

/// Counters the IOProc updates and anyone may read.
#[derive(Default)]
pub struct Status {
    pub cycles: AtomicU64,
    /// Callbacks dropped for invalid time stamps or an unusable buffer.
    pub skipped: AtomicU64,
    pub reanchors: AtomicU64,
    /// Latest phase error, frames, as f64 bits.
    pub error: AtomicU64,
    /// Latest ratio correction, as f64 bits.
    pub correction: AtomicU64,
    /// Timeline positions of the latest clicks, as f64 bits ([`TestSignal::Clicks`]).
    pub clicks: [AtomicU64; 8],
    pub click_count: AtomicU64,
    /// Monitor frames dropped because the queue was full.
    pub monitor_overflows: AtomicU64,
    pub monitor_underruns: AtomicU64,
    /// Smoothed monitor queue level, frames, as f64 bits.
    pub monitor_fill: AtomicU64,
    /// Headphone callbacks run.
    pub monitor_cycles: AtomicU64,
    /// Host times at which the latest clicks reach the headphones'
    /// converters (presentation time plus the device's reported latency),
    /// as f64 bits ([`TestSignal::Clicks`]).
    pub monitor_clicks: [AtomicU64; 8],
    pub monitor_click_count: AtomicU64,
}

/// Everything the IOProc touches. Allocated before IO starts, freed after
/// it stops.
struct Rt {
    processor: AudioProcessor,
    converter: Converter,
    mapping: Arc<shm::Mapping>,
    status: Arc<Status>,
    source_rate: f64,
    ticks_per_frame: f64,
    test: Option<TestSignal>,
    input: Vec<f32>,
    converted: Vec<f32>,
    processed: Vec<f32>,
    /// Monitor output of the engine, and the queue to the headphones.
    monitor: Vec<f32>,
    monitor_tx: rtrb::Producer<f32>,
    /// True while the headphones' IOProc runs.
    monitor_live: Arc<AtomicBool>,
}

unsafe extern "C" fn capture(
    _dev: AudioObjectId,
    _now: *const TimeStamp,
    input: *const AudioBufferList,
    input_time: *const TimeStamp,
    _output: *mut AudioBufferList,
    _output_time: *const TimeStamp,
    client: *mut c_void,
) -> OsStatus {
    // SAFETY: `client` is the Rt given to start_io_proc, alive until the
    // IOProc is stopped; only this thread touches it meanwhile.
    let rt = unsafe { &mut *(client as *mut Rt) };
    // SAFETY: the HAL passes valid buffers and time stamps, or null.
    let (Some(list), Some(time)) = (unsafe { input.as_ref() }, unsafe { input_time.as_ref() })
    else {
        rt.status.skipped.fetch_add(1, Relaxed);
        return NO_ERROR;
    };
    if list.count == 0 || list.buffers[0].data.is_null() || time.flags & TIMES_VALID != TIMES_VALID
    {
        rt.status.skipped.fetch_add(1, Relaxed);
        return NO_ERROR;
    }
    let b = &list.buffers[0];
    let channels = b.channels.max(1) as usize;
    let frames = (b.bytes as usize / 4 / channels).min(rt.input.len());
    // SAFETY: the buffer holds `bytes` bytes of interleaved f32, checked
    // against the stream format before IO started.
    let data = unsafe { std::slice::from_raw_parts(b.data as *const f32, frames * channels) };
    let src = &mut rt.input[..frames];
    for (d, s) in src.iter_mut().zip(data.iter().step_by(channels)) {
        *d = *s;
    }

    let position = time.host_time as f64 / rt.ticks_per_frame;
    if let Some(signal) = rt.test {
        let rate = rt.source_rate;
        for (i, s) in src.iter_mut().enumerate() {
            let n = time.sample_time + i as f64;
            *s = match signal {
                TestSignal::Sine => {
                    0.25 * (std::f64::consts::TAU * 440.0 * (n % rate) / rate).sin() as f32
                }
                TestSignal::Clicks => {
                    if n % rate == 0.0 {
                        let k = rt.status.click_count.fetch_add(1, Relaxed) as usize;
                        let at = position + i as f64 * 48_000.0 / rate;
                        rt.status.clicks[k % 8].store(at.to_bits(), Relaxed);
                        1.0
                    } else {
                        0.0
                    }
                }
            };
        }
    }

    let c = rt.converter.process(
        &rt.input[..frames],
        time.sample_time,
        position,
        &mut rt.converted,
    );
    if c.frames > 0 {
        let n = c.frames;
        let monitor = rt.monitor_live.load(Acquire) && rt.processor.monitor_config().enabled;
        let mon = monitor.then(|| &mut rt.monitor[..n]);
        rt.processor
            .process(&rt.converted[..n], &mut rt.processed[..n], mon);
        rt.mapping.segment().write(c.pos, &rt.processed[..n]);
        if monitor {
            let room = rt.monitor_tx.slots().min(n);
            if let Ok(mut chunk) = rt.monitor_tx.write_chunk(room) {
                let (a, b) = chunk.as_mut_slices();
                let split = a.len();
                a.copy_from_slice(&rt.monitor[..split]);
                b.copy_from_slice(&rt.monitor[split..room]);
                chunk.commit_all();
            }
            if room < n {
                rt.status
                    .monitor_overflows
                    .fetch_add((n - room) as u64, Relaxed);
            }
        }
    }
    let s = &rt.status;
    s.cycles.fetch_add(1, Relaxed);
    s.reanchors.store(rt.converter.reanchors(), Relaxed);
    s.error.store(c.error.to_bits(), Relaxed);
    s.correction
        .store(rt.converter.correction().to_bits(), Relaxed);
    NO_ERROR
}

/// Everything the headphones' IOProc touches.
struct MonitorRt {
    out: MonitorOut,
    mono: Vec<f32>,
    rate: f64,
    status: Arc<Status>,
    /// Records click arrival times ([`TestSignal::Clicks`]).
    test_clicks: bool,
    ticks_per_frame: f64,
    /// The headphones' reported latency beyond their buffer, frames.
    output_latency: f64,
    /// Frames to skip after a detected click, so one click counts once.
    holdoff: usize,
}

unsafe extern "C" fn playback(
    _dev: AudioObjectId,
    _now: *const TimeStamp,
    _input: *const AudioBufferList,
    _input_time: *const TimeStamp,
    output: *mut AudioBufferList,
    output_time: *const TimeStamp,
    client: *mut c_void,
) -> OsStatus {
    // SAFETY: `client` is the MonitorRt given to start_io_proc, alive until
    // the IOProc is stopped; only this thread touches it meanwhile.
    let rt = unsafe { &mut *(client as *mut MonitorRt) };
    // SAFETY: the HAL passes a valid buffer list, or null.
    let Some(list) = (unsafe { output.as_mut() }) else {
        return NO_ERROR;
    };
    if list.count == 0 || list.buffers[0].data.is_null() {
        return NO_ERROR;
    }
    let b = &list.buffers[0];
    let channels = b.channels.max(1) as usize;
    let frames = (b.bytes as usize / 4 / channels).min(rt.mono.len());
    let before = rt.out.underruns();
    rt.out
        .render(&mut rt.mono[..frames], frames as f64 / rt.rate);
    if rt.out.underruns() != before {
        rt.status.monitor_underruns.fetch_add(1, Relaxed);
    }
    rt.status.monitor_cycles.fetch_add(1, Relaxed);
    rt.status
        .monitor_fill
        .store(rt.out.fill().to_bits(), Relaxed);
    // SAFETY: the HAL passes a valid time stamp, or null.
    if rt.test_clicks
        && let Some(time) = unsafe { output_time.as_ref() }
        && time.flags & TIMES_VALID == TIMES_VALID
    {
        for (i, v) in rt.mono[..frames].iter().enumerate() {
            if rt.holdoff > 0 {
                rt.holdoff -= 1;
            } else if v.abs() > 0.01 {
                let at =
                    time.host_time as f64 + (i as f64 + rt.output_latency) * rt.ticks_per_frame;
                let k = rt.status.monitor_click_count.fetch_add(1, Relaxed) as usize;
                rt.status.monitor_clicks[k % 8].store(at.to_bits(), Relaxed);
                rt.holdoff = rt.rate as usize / 2;
            }
        }
    }
    // SAFETY: the buffer holds `bytes` bytes of interleaved f32, checked
    // against the stream format before IO started. The HAL lists further
    // buffers only for non-interleaved formats, which are refused.
    let data = unsafe { std::slice::from_raw_parts_mut(b.data as *mut f32, frames * channels) };
    for (frame, &v) in data.chunks_mut(channels).zip(&rt.mono[..frames]) {
        frame.fill(v);
    }
    NO_ERROR
}

#[derive(Default)]
pub struct MacBackend {
    test: Option<TestSignal>,
}

impl MacBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the captured audio with `signal` (tests only).
    pub fn with_test_signal(signal: TestSignal) -> Self {
        Self { test: Some(signal) }
    }

    fn list(&self, want: fn(AudioObjectId) -> bool) -> Result<Vec<DeviceInfo>, BackendError> {
        let ids = hal::devices().map_err(|e| err("device list", e))?;
        Ok(ids
            .into_iter()
            .filter(|&id| want(id))
            .filter_map(|id| {
                let uid = hal::uid(id).ok()?;
                (uid != model::DEVICE_UID).then(|| DeviceInfo {
                    label: hal::name(id).unwrap_or_else(|_| uid.clone()),
                    id: uid,
                })
            })
            .collect())
    }

    /// Starts capture, returning the running backend with its status.
    pub fn start_mac(
        &mut self,
        processor: AudioProcessor,
        input: Option<&str>,
        monitor_output: Option<&str>,
    ) -> Result<MacRunning, BackendError> {
        let dev = resolve(input)?;
        let cap = prepare(dev)?;
        let generation = RandomState::new().build_hasher().finish() | 1;
        let mapping = Arc::new(
            shm::create(generation).map_err(|e| BackendError(format!("shared memory: {e}")))?,
        );
        let status = Arc::new(Status::default());
        let (monitor_tx, monitor_rx) = rtrb::RingBuffer::new(MONITOR_QUEUE);
        let monitor_live = Arc::new(AtomicBool::new(false));
        let watch = processor.watch();
        let mut rt = Box::new(Rt {
            processor,
            converter: Converter::new(48_000.0, 1),
            mapping: mapping.clone(),
            status: status.clone(),
            source_rate: 48_000.0,
            ticks_per_frame: clock::ticks_per_frame(hal::host_ticks_per_second()),
            test: self.test,
            input: Vec::new(),
            converted: Vec::new(),
            processed: Vec::new(),
            monitor: Vec::new(),
            monitor_tx,
            monitor_live: monitor_live.clone(),
        });
        let info = rt.configure(cap);
        let rt = Box::into_raw(rt);
        // SAFETY: `rt` lives until MacRunning drops, after the IOProc stops.
        let proc_id = match unsafe { hal::start_io_proc(dev, capture, rt as *mut c_void) } {
            Ok(id) => id,
            Err(e) => {
                // SAFETY: the IOProc never started; nothing else holds `rt`.
                drop(unsafe { Box::from_raw(rt) });
                mapping.unlink();
                return Err(err("start capture", e));
            }
        };

        let mic = Arc::new(Mic {
            io: Mutex::new(Io {
                dev,
                proc_id,
                info,
                paused: false,
            }),
            rt,
        });
        let stop = Arc::new(AtomicBool::new(false));
        let headphones = Arc::new(Mutex::new(Headphones {
            uid: monitor_output.map(str::to_owned),
            changed: false,
        }));
        let monitor_latency = Arc::new(AtomicUsize::new(0));
        let control = {
            let mut c = Control {
                watch,
                mic: mic.clone(),
                standby: engine::standby::Policy::new(),
                readers: None,
                mapping: mapping.clone(),
                stop: stop.clone(),
                headphones: headphones.clone(),
                live: monitor_live,
                queue: Some(monitor_rx),
                running: None,
                failed: false,
                test: self.test,
                status: status.clone(),
                monitor_latency: monitor_latency.clone(),
                capture_frames: info.capture_frames,
                capture_latency: info.capture_frames + info.io_latency
                    - model::TRANSPORT_LATENCY as usize,
            };
            std::thread::Builder::new()
                .name("voicestrip-control".into())
                .spawn(move || c.run())
                .map_err(|e| BackendError(format!("control thread: {e}")))?
        };
        Ok(MacRunning {
            mic,
            mapping,
            status,
            stop,
            control: Some(control),
            headphones,
            monitor_latency,
        })
    }
}

/// The device for `input` (`None`: the system default), never Voice Strip.
fn resolve(input: Option<&str>) -> Result<AudioObjectId, BackendError> {
    let dev = match input {
        Some(uid) => hal::find(uid)
            .map_err(|e| err("device list", e))?
            .ok_or_else(|| BackendError(format!("no input device {uid}")))?,
        None => hal::default_input().map_err(|e| err("default input", e))?,
    };
    if hal::uid(dev).is_ok_and(|u| u == model::DEVICE_UID) {
        return Err(BackendError(
            "the input is Voice Strip itself: choose a microphone".into(),
        ));
    }
    Ok(dev)
}

/// What capturing from one device needs, prepared before its IOProc runs.
struct Capture {
    frames: usize,
    rate: f64,
    input_latency: usize,
}

/// Checks the device's format and asks for a small IO buffer.
fn prepare(dev: AudioObjectId) -> Result<Capture, BackendError> {
    let format = hal::input_format(dev).map_err(|e| err("input format", e))?;
    check_format(&format)?;
    let rate = hal::nominal_rate(dev).map_err(|e| err("sample rate", e))?;
    let frames = hal::set_buffer_frame_size(dev, CAPTURE_FRAMES)
        .or_else(|_| hal::buffer_frame_size(dev))
        .map_err(|e| err("buffer size", e))? as usize;
    Ok(Capture {
        frames,
        rate,
        input_latency: hal::input_latency(dev) as usize,
    })
}

/// The running capture's parameters.
#[derive(Clone, Copy, Debug)]
pub struct CaptureInfo {
    pub capture_frames: usize,
    pub source_rate: f64,
    /// Converter delay, frames on the timeline.
    pub converter_delay: f64,
    /// Main-path latency outside the engine: the microphone's own latency,
    /// the converter delay and the transport margin to the virtual device.
    /// The capture buffer itself is covered by the margin.
    pub io_latency: usize,
}

impl Rt {
    /// Sizes everything for `cap`. Allocates: only while the IOProc is not
    /// running.
    fn configure(&mut self, cap: Capture) -> CaptureInfo {
        // Inputs larger than the granted size are cut, so leave room for a
        // device that later delivers more.
        let max_input = cap.frames.max(4096);
        self.converter = Converter::new(cap.rate, max_input);
        let max_output = self.converter.max_output();
        self.input = vec![0.0; max_input];
        self.converted = vec![0.0; max_output];
        self.processed = vec![0.0; max_output];
        self.monitor = vec![0.0; max_output];
        self.source_rate = cap.rate;
        // The largest block the engine sees in normal running: one capture
        // buffer converted at the highest ratio.
        let engine_block = (cap.frames as f64 * 48_000.0 / cap.rate * 1.002).ceil() as usize + 2;
        self.processor.set_host_block_size(Some(engine_block));
        let converter_delay = self.converter.delay();
        CaptureInfo {
            capture_frames: cap.frames,
            source_rate: cap.rate,
            converter_delay,
            io_latency: cap.input_latency
                + converter_delay.round() as usize
                + model::TRANSPORT_LATENCY as usize,
        }
    }
}

fn err(what: &str, status: OsStatus) -> BackendError {
    BackendError(format!("{what}: Core Audio error {status}"))
}

/// The capture IOProc reads interleaved 32-bit float, the HAL's usual
/// virtual format.
fn check_format(f: &StreamBasicDescription) -> Result<(), BackendError> {
    if f.format_id == FORMAT_LINEAR_PCM
        && f.format_flags & FLAG_FLOAT != 0
        && f.format_flags & FLAG_NON_INTERLEAVED == 0
        && f.bits_per_channel == 32
        && f.channels_per_frame > 0
    {
        Ok(())
    } else {
        Err(BackendError(format!("unsupported input format {f:?}")))
    }
}

impl AudioBackend for MacBackend {
    fn list_inputs(&self) -> Result<Vec<DeviceInfo>, BackendError> {
        self.list(hal::has_input)
    }

    fn list_outputs(&self) -> Result<Vec<DeviceInfo>, BackendError> {
        self.list(hal::has_output)
    }

    fn start(
        &mut self,
        processor: AudioProcessor,
        input: Option<&str>,
        monitor_output: Option<&str>,
    ) -> Result<Box<dyn RunningBackend>, BackendError> {
        Ok(Box::new(self.start_mac(
            processor,
            input,
            monitor_output,
        )?))
    }
}

/// The device being captured and its IOProc.
struct Io {
    dev: AudioObjectId,
    proc_id: IoProcId,
    info: CaptureInfo,
    /// Capture paused: the IOProc is stopped and destroyed, so the
    /// microphone is released.
    paused: bool,
}

/// A running capture. Dropping it stops IO and removes the segment.
pub struct MacRunning {
    mic: Arc<Mic>,
    mapping: Arc<shm::Mapping>,
    status: Arc<Status>,
    stop: Arc<AtomicBool>,
    control: Option<JoinHandle<()>>,
    headphones: Arc<Mutex<Headphones>>,
    /// Monitor path latency outside the engine, frames at 48 kHz.
    monitor_latency: Arc<AtomicUsize>,
}

/// The microphone's IOProc and its realtime state, shared by the running
/// handle (device switches) and the control thread (standby).
struct Mic {
    io: Mutex<Io>,
    rt: *mut Rt,
}

// SAFETY: `rt` is only touched by the IOProc while it runs, and under the
// `io` lock while it is stopped.
unsafe impl Send for Mic {}
unsafe impl Sync for Mic {}

impl Mic {
    /// Stops (`true`) or restarts the IOProc. Stopped, the microphone is
    /// released. Returns false when a restart failed.
    fn set_paused(&self, paused: bool) -> bool {
        let mut io = self.io.lock().unwrap_or_else(|e| e.into_inner());
        if io.paused == paused {
            return true;
        }
        if paused {
            // SAFETY: from start_io_proc on `io.dev`; it is not called again.
            unsafe { hal::stop_io_proc(io.dev, io.proc_id) };
            io.paused = true;
            return true;
        }
        // SAFETY: `rt` outlives the IOProc (freed in Drop after stopping it).
        match unsafe { hal::start_io_proc(io.dev, capture, self.rt as *mut c_void) } {
            Ok(proc_id) => {
                io.proc_id = proc_id;
                io.paused = false;
                true
            }
            Err(e) => {
                eprintln!("voice-strip: cannot resume the microphone: {e}");
                false
            }
        }
    }
}

impl Drop for Mic {
    fn drop(&mut self) {
        // The last owner: the running handle stopped the IOProc first.
        let io = self.io.get_mut().unwrap_or_else(|e| e.into_inner());
        if !io.paused {
            // SAFETY: from start_io_proc on `dev`; never called again after.
            unsafe { hal::stop_io_proc(io.dev, io.proc_id) };
        }
        // SAFETY: from Box::into_raw; the IOProc no longer uses it.
        drop(unsafe { Box::from_raw(self.rt) });
    }
}

impl MacRunning {
    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn info(&self) -> CaptureInfo {
        self.mic
            .io
            .lock()
            .map_or_else(|e| e.into_inner().info, |io| io.info)
    }

    /// Moves capture to `dev`: stops the IOProc, resizes for the new device
    /// and starts it there. On failure capture goes back to the old device.
    fn switch(&self, dev: AudioObjectId) -> Result<(), BackendError> {
        let cap = prepare(dev)?;
        let mut io = self.mic.io.lock().unwrap_or_else(|e| e.into_inner());
        if io.paused {
            // Resumed on the new device; nothing runs meanwhile.
            // SAFETY: the IOProc is stopped while paused, so `rt` is ours.
            io.info = unsafe { &mut *self.mic.rt }.configure(cap);
            io.dev = dev;
            return Ok(());
        }
        // SAFETY: from start_io_proc on `io.dev`; afterwards the IOProc no
        // longer runs, so `rt` is ours until it starts again.
        unsafe { hal::stop_io_proc(io.dev, io.proc_id) };
        // SAFETY: as above; nothing else touches `rt` under the lock.
        let rt = unsafe { &mut *self.mic.rt };
        let info = rt.configure(cap);
        // SAFETY: `rt` outlives the IOProc (freed in Drop after stopping it).
        match unsafe { hal::start_io_proc(dev, capture, self.mic.rt as *mut c_void) } {
            Ok(proc_id) => {
                *io = Io {
                    dev,
                    proc_id,
                    info,
                    paused: false,
                };
                Ok(())
            }
            Err(e) => {
                let old = io.dev;
                if let Ok(cap) = prepare(old) {
                    io.info = rt.configure(cap);
                }
                // SAFETY: as above.
                match unsafe { hal::start_io_proc(old, capture, self.mic.rt as *mut c_void) } {
                    Ok(proc_id) => io.proc_id = proc_id,
                    // Nothing runs now: later stops must not touch the
                    // destroyed IOProc, and a resume starts a new one.
                    Err(_) => io.paused = true,
                }
                Err(err("start capture", e))
            }
        }
    }
}

impl RunningBackend for MacRunning {
    fn virtual_device_name(&self) -> &str {
        model::DEVICE_NAME
    }

    fn io_latency_samples(&self) -> (usize, usize) {
        (self.info().io_latency, self.monitor_latency.load(Relaxed))
    }

    fn set_monitor_output(&self, output: Option<&str>) {
        let mut h = self.headphones.lock().unwrap_or_else(|e| e.into_inner());
        h.uid = output.map(str::to_owned);
        h.changed = true;
    }

    fn set_input(&self, input: Option<&str>) {
        if let Err(e) = resolve(input).and_then(|dev| self.switch(dev)) {
            eprintln!("voice-strip: cannot switch microphone: {e}");
        }
    }
}

impl Drop for MacRunning {
    fn drop(&mut self) {
        // The control thread first: it shares the microphone and stops the
        // headphones' IOProc on its way out.
        self.stop.store(true, Relaxed);
        if let Some(h) = self.control.take() {
            let _ = h.join();
        }
        // Stop the microphone before the segment goes; the realtime state
        // is freed when the last owner, this handle, drops the `Mic`.
        self.mic.set_paused(true);
        self.mapping.unlink();
    }
}

/// The headphones the monitor plays on (`None`: the system default).
struct Headphones {
    uid: Option<String>,
    changed: bool,
}

/// The headphones' IOProc while it runs.
struct Playing {
    dev: AudioObjectId,
    proc_id: IoProcId,
    rt: *mut MonitorRt,
}

/// Off the realtime threads: keeps the segment's heartbeat, and starts or
/// stops the headphones' IOProc as the UI turns monitoring on and off or
/// picks other headphones.
struct Control {
    watch: ProcessorWatch,
    mic: Arc<Mic>,
    standby: engine::standby::Policy,
    /// Whether an application reads Voice Strip, asked twice a second.
    readers: Option<bool>,
    mapping: Arc<shm::Mapping>,
    stop: Arc<AtomicBool>,
    headphones: Arc<Mutex<Headphones>>,
    live: Arc<AtomicBool>,
    /// The queue's reading end while no headphone IOProc owns it.
    queue: Option<rtrb::Consumer<f32>>,
    running: Option<Playing>,
    /// The last start failed; see `run`.
    failed: bool,
    test: Option<TestSignal>,
    status: Arc<Status>,
    monitor_latency: Arc<AtomicUsize>,
    /// The microphone's IO buffer, frames at 48 kHz.
    capture_frames: usize,
    /// Microphone latency, capture buffer and converter delay, frames.
    capture_latency: usize,
}

// SAFETY: the raw MonitorRt pointer is only handed to the HAL and freed by
// this thread after its IOProc stopped.
unsafe impl Send for Control {}

impl Control {
    fn run(&mut self) {
        let mut tick = 0u32;
        while !self.stop.load(Relaxed) {
            if tick.is_multiple_of(4) {
                self.mapping
                    .segment()
                    .header
                    .heartbeat
                    .fetch_add(1, Relaxed);
            }
            if tick.is_multiple_of(20) {
                self.readers = hal::find(model::DEVICE_UID)
                    .ok()
                    .flatten()
                    .and_then(|d| hal::is_running_somewhere(d).ok());
            }
            if let Some(standby) = self.standby.update(self.readers, self.watch.capture_held()) {
                if self.mic.set_paused(standby) {
                    self.watch.set_standby(standby);
                } else {
                    // A failed resume: stay in standby, retry next pass.
                    self.standby.revert();
                }
            }
            tick = tick.wrapping_add(1);
            let changed = std::mem::take(
                &mut self
                    .headphones
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .changed,
            );
            let wanted = self.watch.monitor_wanted();
            if changed || !wanted {
                // A failed start is retried only for other headphones or a
                // new request.
                self.failed = false;
            }
            if self.running.is_some() && (!wanted || changed) {
                self.stop_playing();
            }
            if wanted && self.running.is_none() && !self.failed {
                let uid = self
                    .headphones
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .uid
                    .clone();
                if let Err(e) = self.start_playing(uid.as_deref()) {
                    eprintln!("voice-strip: headphone monitor: {e}");
                    self.failed = true;
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        self.stop_playing();
    }

    fn start_playing(&mut self, uid: Option<&str>) -> Result<(), BackendError> {
        let dev = match uid {
            Some(uid) => hal::find(uid)
                .map_err(|e| err("device list", e))?
                .ok_or_else(|| BackendError(format!("no output device {uid}")))?,
            None => hal::default_output().map_err(|e| err("default output", e))?,
        };
        let format = hal::output_format(dev).map_err(|e| err("output format", e))?;
        check_format(&format)?;
        let rate = hal::nominal_rate(dev).map_err(|e| err("sample rate", e))?;
        let frames = hal::set_buffer_frame_size(dev, CAPTURE_FRAMES)
            .or_else(|_| hal::buffer_frame_size(dev))
            .map_err(|e| err("buffer size", e))? as usize;
        // Only called with no headphone IOProc running, so the queue is
        // here: the start returned it, or stop_playing did.
        let mut queue = self
            .queue
            .take()
            .expect("monitor queue owned while stopped");
        // Start from an empty queue, so old audio never plays.
        let stale = queue.slots();
        if let Ok(chunk) = queue.read_chunk(stale) {
            chunk.commit_all();
        }
        let max = frames.max(4096);
        let to_48k = |n: usize| (n as f64 * 48_000.0 / rate).round() as usize;
        let target = target_fill(self.capture_frames.max(to_48k(frames)));
        let out = MonitorOut::new(queue, target, rate, max);
        let delay = out.delay();
        let rt = Box::into_raw(Box::new(MonitorRt {
            out,
            mono: vec![0.0; max],
            rate,
            status: self.status.clone(),
            test_clicks: self.test == Some(TestSignal::Clicks),
            ticks_per_frame: hal::host_ticks_per_second() / rate,
            output_latency: f64::from(hal::output_latency(dev)),
            holdoff: 0,
        }));
        // SAFETY: `rt` lives until stop_playing frees it after stopping
        // the IOProc.
        match unsafe { hal::start_io_proc(dev, playback, rt as *mut c_void) } {
            Ok(proc_id) => {
                self.running = Some(Playing { dev, proc_id, rt });
                self.live.store(true, Release);
                self.monitor_latency.store(
                    self.capture_latency
                        + target as usize
                        + to_48k(delay + frames + hal::output_latency(dev) as usize),
                    Relaxed,
                );
                Ok(())
            }
            Err(e) => {
                // SAFETY: the IOProc never started; nothing else holds `rt`.
                let rt = unsafe { Box::from_raw(rt) };
                self.queue = Some(rt.out.into_queue());
                Err(err("start headphones", e))
            }
        }
    }

    fn stop_playing(&mut self) {
        let Some(p) = self.running.take() else {
            return;
        };
        self.live.store(false, Release);
        // SAFETY: from start_io_proc on `p.dev`; afterwards the IOProc is
        // never called again.
        unsafe { hal::stop_io_proc(p.dev, p.proc_id) };
        // SAFETY: from Box::into_raw; the IOProc no longer uses it.
        let rt = unsafe { Box::from_raw(p.rt) };
        self.queue = Some(rt.out.into_queue());
        self.monitor_latency.store(0, Relaxed);
    }
}
