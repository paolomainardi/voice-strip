use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use dsp::{Chain, ChainSettings, PathKind};

use crate::meters::{MeterFrame, rectified_average};

/// Largest host block [`AudioProcessor::process`] accepts, samples.
pub const MAX_HOST_BLOCK: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MonitorConfig {
    pub enabled: bool,
    /// Include neural denoise in the monitor path (+~20 ms).
    pub with_denoise: bool,
    /// Monitor output level, linear gain.
    pub level: f32,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            with_denoise: false,
            level: 1.0,
        }
    }
}

/// State shared between the UI handle, the audio processor and the backend.
/// Atomics only.
struct Shared {
    running: AtomicBool,
    /// Latest `MonitorConfig::enabled` requested by the UI. Backends poll it
    /// from a non-realtime thread to create or drop the monitor stream.
    monitor_wanted: AtomicBool,
    /// Main-path DSP latency (chain), samples.
    main_latency: AtomicUsize,
    /// Monitor-path DSP latency, samples.
    monitor_latency: AtomicUsize,
    /// Fixed host block size, 0 when variable or unknown.
    host_block: AtomicUsize,
}

/// UI-side handle. Lives on a non-realtime thread.
pub struct EngineHandle {
    settings_tx: rtrb::Producer<ChainSettings>,
    chain_tx: rtrb::Producer<Box<Chain>>,
    retired_rx: rtrb::Consumer<Box<Chain>>,
    vad_tx: rtrb::Producer<Box<dyn dsp::VoiceDetector>>,
    monitor_tx: rtrb::Producer<MonitorConfig>,
    meters_rx: rtrb::Consumer<MeterFrame>,
    /// Output samples for the spectrum analyzer (main path, post-chain).
    pub scope_out_rx: rtrb::Consumer<f32>,
    /// Input samples for the spectrum analyzer (pre-chain).
    pub scope_in_rx: rtrb::Consumer<f32>,
    shared: Arc<Shared>,
}

impl EngineHandle {
    /// Queues new chain settings. Returns false when the queue is full
    /// (the audio thread is not running); the caller may retry.
    pub fn set_settings(&mut self, s: ChainSettings) -> bool {
        self.settings_tx.push(s).is_ok()
    }

    pub fn set_monitor(&mut self, m: MonitorConfig) -> bool {
        self.shared
            .monitor_wanted
            .store(m.enabled, Ordering::Release);
        self.monitor_tx.push(m).is_ok()
    }

    /// Pops the oldest pending meter frame.
    pub fn pop_meter(&mut self) -> Option<MeterFrame> {
        self.meters_rx.pop().ok()
    }

    /// Drains pending meter frames, returning the most recent one.
    /// Replaces the main chain (or the monitor chain, when `chain.path()` is
    /// `PathKind::Monitor`), for example to add or remove the neural
    /// denoiser (which changes the chain latency). The audio thread swaps it
    /// in at the next block and hands the old chain back, so it is dropped
    /// here, never on the audio thread. Returns the chain if the queue is
    /// full. The swap restarts filter and delay state: a short discontinuity
    /// is expected.
    pub fn replace_main_chain(&mut self, chain: Box<Chain>) -> Result<(), Box<Chain>> {
        self.drop_retired();
        self.chain_tx
            .push(chain)
            .map_err(|rtrb::PushError::Full(c)| c)
    }

    /// Hands the voice detector to the audio thread, which feeds it the
    /// input and steers the main chain with it. Set once; returns the
    /// detector if one is already queued.
    pub fn set_voice_detector(
        &mut self,
        vad: Box<dyn dsp::VoiceDetector>,
    ) -> Result<(), Box<dyn dsp::VoiceDetector>> {
        self.vad_tx.push(vad).map_err(|rtrb::PushError::Full(v)| v)
    }

    /// Drops chains the audio thread has swapped out.
    pub fn drop_retired(&mut self) {
        while self.retired_rx.pop().is_ok() {}
    }

    pub fn latest_meters(&mut self) -> Option<MeterFrame> {
        self.drop_retired();
        let mut last = None;
        while let Ok(f) = self.meters_rx.pop() {
            last = Some(f);
        }
        last
    }

    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::Relaxed)
    }

    /// Latency of the main path in samples: chain DSP latency. Backend I/O
    /// latency excluded.
    pub fn main_latency_samples(&self) -> usize {
        self.shared.main_latency.load(Ordering::Acquire)
    }

    /// Latency of the monitor path in samples: chain DSP latency. Backend
    /// I/O latency excluded.
    pub fn monitor_latency_samples(&self) -> usize {
        self.shared.monitor_latency.load(Ordering::Acquire)
    }

    /// Host block size the backend declared, `None` when variable or not
    /// started yet. Chains with a neural stage size their handoff from it.
    pub fn host_block(&self) -> Option<usize> {
        match self.shared.host_block.load(Ordering::Acquire) {
            0 => None,
            n => Some(n),
        }
    }
}

/// Non-realtime view of the processor state for backends: whether the UI
/// wants the monitor path and the current latencies. Atomics only.
#[derive(Clone)]
pub struct ProcessorWatch(Arc<Shared>);

impl ProcessorWatch {
    pub fn monitor_wanted(&self) -> bool {
        self.0.monitor_wanted.load(Ordering::Acquire)
    }

    pub fn main_latency_samples(&self) -> usize {
        self.0.main_latency.load(Ordering::Acquire)
    }

    pub fn monitor_latency_samples(&self) -> usize {
        self.0.monitor_latency.load(Ordering::Acquire)
    }
}

/// Audio-thread side. Owned by the backend's realtime callback.
pub struct AudioProcessor {
    main: Box<Chain>,
    /// A swapped-out chain waiting for room in the retired queue.
    spare: Option<Box<Chain>>,
    chain_rx: rtrb::Consumer<Box<Chain>>,
    retired_tx: rtrb::Producer<Box<Chain>>,
    vad_rx: rtrb::Consumer<Box<dyn dsp::VoiceDetector>>,
    vad: Option<Box<dyn dsp::VoiceDetector>>,
    settings: ChainSettings,
    monitor: Box<Chain>,
    /// Main-path meters gathered until a meter frame's worth of samples.
    acc: MeterAcc,
    settings_rx: rtrb::Consumer<ChainSettings>,
    monitor_rx: rtrb::Consumer<MonitorConfig>,
    meters_tx: rtrb::Producer<MeterFrame>,
    scope_in_tx: rtrb::Producer<f32>,
    scope_out_tx: rtrb::Producer<f32>,
    monitor_cfg: MonitorConfig,
    shared: Arc<Shared>,
}

pub fn new_engine() -> (EngineHandle, AudioProcessor) {
    new_engine_with(None)
}

/// Like [`new_engine`], with a neural denoiser in the main chain. Its
/// latency (one frame of worker handoff plus the model) is included in the
/// reported main latency.
pub fn new_engine_with(
    neural: Option<Box<dyn dsp::FrameDenoiser>>,
) -> (EngineHandle, AudioProcessor) {
    let (settings_tx, settings_rx) = rtrb::RingBuffer::new(16);
    let (monitor_tx, monitor_rx) = rtrb::RingBuffer::new(16);
    let (meters_tx, meters_rx) = rtrb::RingBuffer::new(256);
    let (scope_in_tx, scope_in_rx) = rtrb::RingBuffer::new(dsp::SAMPLE_RATE as usize);
    let (scope_out_tx, scope_out_rx) = rtrb::RingBuffer::new(dsp::SAMPLE_RATE as usize);
    let main = Box::new(Chain::with_denoiser(
        PathKind::Main,
        dsp::BLOCK_MAIN,
        neural,
    ));
    let (chain_tx, chain_rx) = rtrb::RingBuffer::new(2);
    let (retired_tx, retired_rx) = rtrb::RingBuffer::new(4);
    let (vad_tx, vad_rx) = rtrb::RingBuffer::new(1);
    let monitor = Box::new(Chain::new(PathKind::Monitor, dsp::BLOCK_MONITOR));
    let shared = Arc::new(Shared {
        running: AtomicBool::new(false),
        monitor_wanted: AtomicBool::new(false),
        main_latency: AtomicUsize::new(main.latency_samples()),
        monitor_latency: AtomicUsize::new(monitor.latency_samples()),
        host_block: AtomicUsize::new(0),
    });
    (
        EngineHandle {
            settings_tx,
            chain_tx,
            retired_rx,
            vad_tx,
            monitor_tx,
            meters_rx,
            scope_out_rx,
            scope_in_rx,
            shared: shared.clone(),
        },
        AudioProcessor {
            main,
            spare: None,
            chain_rx,
            retired_tx,
            vad_rx,
            vad: None,
            settings: ChainSettings::default(),
            monitor,
            acc: MeterAcc::default(),
            settings_rx,
            monitor_rx,
            meters_tx,
            scope_in_tx,
            scope_out_tx,
            monitor_cfg: MonitorConfig::default(),
            shared,
        },
    )
}

impl AudioProcessor {
    pub fn monitor_config(&self) -> MonitorConfig {
        self.monitor_cfg
    }

    pub fn watch(&self) -> ProcessorWatch {
        ProcessorWatch(self.shared.clone())
    }

    pub fn set_running(&self, running: bool) {
        self.shared.running.store(running, Ordering::Relaxed);
    }

    /// Declares the host block size, `None` for variable: the largest
    /// block the backend passes. Processing works with any size; chains
    /// with a neural stage size their handoff from it. Call before starting,
    /// or from the audio thread on a quantum change. Realtime-safe.
    pub fn set_host_block_size(&mut self, n: Option<usize>) {
        let n = n.filter(|&n| n > 0 && n <= MAX_HOST_BLOCK).unwrap_or(0);
        self.shared.host_block.store(n, Ordering::Release);
    }

    /// Processes one host block. `input`, `main_out` and `monitor_out` have
    /// the same length (any size up to [`MAX_HOST_BLOCK`]).
    ///
    /// Both paths process the host block directly, with no re-blocking
    /// delay: the main chain on chunks of at most `dsp::BLOCK_MAIN`, the
    /// monitor chain on chunks of at most `dsp::BLOCK_MONITOR`. The monitor
    /// path costs nothing when `monitor_out` is `None`. Meter frames go out
    /// every `dsp::BLOCK_MAIN` samples (100 Hz).
    /// Realtime-safe.
    pub fn process(
        &mut self,
        input: &[f32],
        main_out: &mut [f32],
        monitor_out: Option<&mut [f32]>,
    ) {
        let _ftz = crate::denormals::FlushDenormals::new();
        if let Some(c) = self.spare.take()
            && let Err(rtrb::PushError::Full(c)) = self.retired_tx.push(c)
        {
            self.spare = Some(c);
        }
        let mut settings_changed = false;
        while let Ok(s) = self.settings_rx.pop() {
            self.main.set_settings(&s);
            self.monitor.set_settings(&s);
            self.settings = s;
            settings_changed = true;
        }
        if self.spare.is_none()
            && let Ok(mut chain) = self.chain_rx.pop()
        {
            chain.set_settings(&self.settings);
            match chain.path() {
                PathKind::Main => {
                    chain.seed_room(self.main.room_db());
                    std::mem::swap(&mut self.main, &mut chain);
                }
                PathKind::Monitor => std::mem::swap(&mut self.monitor, &mut chain),
            }
            // The queue holds 4 and the UI drains it every frame; if it is
            // ever full, park the old chain and retry on the next block, so
            // it is never freed here.
            if let Err(rtrb::PushError::Full(c)) = self.retired_tx.push(chain) {
                self.spare = Some(c);
            }
            settings_changed = true;
        }
        while let Ok(m) = self.monitor_rx.pop() {
            self.monitor_cfg = m;
        }
        if self.vad.is_none()
            && let Ok(v) = self.vad_rx.pop()
        {
            self.vad = Some(v);
        }
        if settings_changed {
            self.publish_latency();
        }

        if let Some(out) = monitor_out {
            self.process_monitor(input, out);
        }

        let n = input.len().min(main_out.len());
        let main_out = &mut main_out[..n];
        main_out.copy_from_slice(&input[..n]);
        let Self {
            main,
            vad,
            acc,
            meters_tx,
            scope_in_tx,
            scope_out_tx,
            ..
        } = self;
        for block in main_out.chunks_mut(dsp::BLOCK_MAIN) {
            push_slice(scope_in_tx, block);
            acc.input(block);
            if let Some(v) = vad {
                v.push(block);
                main.set_speech_probability(v.speech_probability());
            }
            let chain_meters = main.process(block);
            push_slice(scope_out_tx, block);
            acc.output(block, &chain_meters);
            if acc.samples >= dsp::BLOCK_MAIN {
                let models = ModelMeters {
                    vad_inference_ms: vad.as_ref().and_then(|v| v.inference_ms()),
                    denoise_inference_ms: main.denoise_inference_ms(),
                };
                let _ = meters_tx.push(acc.frame(main.denoise_missed_frames(), models));
            }
        }
    }

    fn process_monitor(&mut self, input: &[f32], out: &mut [f32]) {
        let n = input.len().min(out.len());
        let out = &mut out[..n];
        if !self.monitor_cfg.enabled {
            out.fill(0.0);
            return;
        }
        out.copy_from_slice(&input[..n]);
        let g = self.monitor_cfg.level;
        for chunk in out.chunks_mut(dsp::BLOCK_MONITOR) {
            self.monitor.process(chunk);
            for x in chunk.iter_mut() {
                *x *= g;
            }
        }
    }

    fn publish_latency(&self) {
        self.shared
            .main_latency
            .store(self.main.latency_samples(), Ordering::Release);
        self.shared
            .monitor_latency
            .store(self.monitor.latency_samples(), Ordering::Release);
    }
}

/// Model run times for a meter frame.
struct ModelMeters {
    vad_inference_ms: Option<f32>,
    denoise_inference_ms: Option<f32>,
}

/// Main-path meter values over the samples since the last meter frame.
#[derive(Default)]
struct MeterAcc {
    samples: usize,
    in_peak: f32,
    in_sq: f32,
    /// Sum of the blocks' rectified averages, weighted by their length.
    in_avg: f32,
    out_sq: f32,
    out_avg: f32,
    chain: dsp::ChainMeters,
}

impl MeterAcc {
    fn input(&mut self, block: &[f32]) {
        for &x in block {
            self.in_peak = self.in_peak.max(x.abs());
            self.in_sq += x * x;
        }
        self.in_avg += rectified_average(block) * block.len() as f32;
    }

    /// Adds the processed block; the chain's loudness and gain reduction are
    /// the latest, its true peak the highest.
    fn output(&mut self, block: &[f32], m: &dsp::ChainMeters) {
        self.out_sq += block.iter().map(|x| x * x).sum::<f32>();
        self.out_avg += rectified_average(block) * block.len() as f32;
        let true_peak_db = self.chain.true_peak_db.max(m.true_peak_db);
        self.chain = dsp::ChainMeters { true_peak_db, ..*m };
        self.samples += block.len();
    }

    /// The frame for the samples gathered so far, at least one frame's
    /// worth; starts a new one. The samples beyond one frame carry over, so
    /// frames keep a 100 Hz rate whatever the block size.
    fn frame(&mut self, denoise_missed_frames: u64, models: ModelMeters) -> MeterFrame {
        let n = self.samples as f32;
        let frame = MeterFrame {
            in_peak_db: to_db(self.in_peak),
            in_rms_db: to_db((self.in_sq / n).sqrt()),
            out_rms_db: to_db((self.out_sq / n).sqrt()),
            in_avg_db: to_db(self.in_avg / n),
            out_avg_db: to_db(self.out_avg / n),
            lufs_momentary: self.chain.lufs_momentary,
            lufs_short_term: self.chain.lufs_short_term,
            true_peak_db: self.chain.true_peak_db,
            gain_reduction_db: self.chain.gain_reduction_db,
            denoise_missed_frames,
            speech_probability: self.chain.speech_probability,
            room_db: self.chain.room_db,
            vad_inference_ms: models.vad_inference_ms,
            denoise_inference_ms: models.denoise_inference_ms,
        };
        *self = Self {
            samples: self.samples - dsp::BLOCK_MAIN,
            ..Self::default()
        };
        frame
    }
}

fn push_slice(tx: &mut rtrb::Producer<f32>, data: &[f32]) {
    let n = data.len().min(tx.slots());
    if let Ok(mut chunk) = tx.write_chunk_uninit(n) {
        let (a, b) = chunk.as_mut_slices();
        for (dst, &src) in a.iter_mut().chain(b.iter_mut()).zip(data) {
            dst.write(src);
        }
        // SAFETY: all `n` slots were written above.
        unsafe { chunk.commit_all() };
    }
}

fn to_db(x: f32) -> f32 {
    if x > 0.0 {
        20.0 * x.log10()
    } else {
        f32::NEG_INFINITY
    }
}
