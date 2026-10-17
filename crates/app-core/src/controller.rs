use dsp::{ChainSettings, Module};
use engine::{EngineHandle, MeterFrame, MonitorConfig};

use crate::calibration::{Calibrator, Phase, SILENCE_SECONDS, SPEECH_SECONDS};
use crate::presets::{DenoiseLevel, PresetId, builtin};
use crate::tuning::{VoiceProfile, settings_for};
use dsp::stages::denoise::handoff_samples;

/// Everything a frontend renders. Frontends read it every frame and send
/// user actions back through [`Controller`] methods.
#[derive(Clone, Debug, PartialEq)]
pub struct UiState {
    pub on: bool,
    /// Others hear silence (virtual microphone and headphone monitor).
    pub muted: bool,
    /// A/B: true plays the original (unprocessed) signal.
    pub ab_original: bool,
    pub preset: PresetId,
    /// Intensity knob, 0..=100.
    pub intensity: u8,
    /// Neural noise removal; `None` is off (lower latency).
    pub neural_denoise: Option<DenoiseLevel>,
    pub monitor: MonitorConfig,
    pub module_enabled: [bool; 9],
    pub meters: MeterFrame,
    /// Main path latency (DSP + I/O), ms.
    pub mic_latency_ms: f32,
    /// Monitor path latency (DSP + I/O), ms.
    pub monitor_latency_ms: f32,
    pub input_device: String,
    /// Headphones used by the monitor path; empty means the system default.
    pub output_device: String,
    pub virtual_device: String,
    /// Settings currently sent to the engine, for the advanced view's
    /// module cards.
    pub chain: ChainSettings,
    /// Name of the active voice profile (person plus microphone).
    pub voice_profile: String,
    /// Process CPU load of the audio thread, percent of one core.
    pub cpu_percent: f32,
    /// Calibration in progress or its last outcome; `None` when idle.
    pub calibration: Option<CalibrationStatus>,
    /// A problem the user should know about (for example noise removal
    /// switched off because the denoiser stopped keeping up).
    pub notice: Option<String>,
    /// The noise removal model, which is downloaded only when the user
    /// agrees to it.
    pub model: ModelState,
    /// What waits for the model: the host shows the download dialog while
    /// this is set, and applies it once the model is ready.
    pub model_prompt: Option<ModelUse>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ModelState {
    Missing,
    /// Bytes received so far.
    Downloading(u64),
    /// The last download failed, with the reason.
    Failed(String),
    Ready,
}

/// A feature that needs the noise removal model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelUse {
    Noise(DenoiseLevel),
    /// Noise removal in the headphone monitor.
    MonitorDenoise,
}

/// What the UI shows while calibrating.
#[derive(Clone, Debug, PartialEq)]
pub struct CalibrationStatus {
    pub phase: Phase,
    /// Seconds left in the current phase.
    pub seconds_left: f32,
    /// Outcome text once done (success or the reason it failed).
    pub message: Option<String>,
    pub error: bool,
}

impl CalibrationStatus {
    /// Short label for the Recalibrate button.
    pub fn label(&self) -> String {
        match (&self.phase, &self.message) {
            (Phase::Silence, _) => format!("Stay quiet... {:.0} s", self.seconds_left.ceil()),
            (Phase::Speech, _) => format!("Now speak... {:.0} s", self.seconds_left.ceil()),
            (Phase::Done, Some(m)) => m.clone(),
            (Phase::Done, None) => "Recalibrate (15 s)".into(),
        }
    }
}

pub struct Controller {
    engine: EngineHandle,
    state: UiState,
    profile: VoiceProfile,
    /// Whether the engine's main chain currently contains the neural stage.
    chain_has_neural: bool,
    /// Device latency reported by the backend, ms.
    io_latency_ms: f32,
    calibrator: Option<Calibrator>,
    /// Handoff of the neural stage in the running chain, samples.
    chain_handoff: usize,
    /// Model loading in the background, with the handoff it is built for.
    loading: Option<(std::sync::mpsc::Receiver<LoadResult>, usize)>,
    /// Same three, for the headphone monitor chain.
    monitor_has_neural: bool,
    monitor_handoff: usize,
    monitor_loading: Option<(std::sync::mpsc::Receiver<LoadResult>, usize)>,
    io_monitor_ms: f32,
    /// Chains built but not yet accepted by the engine (queue full or audio
    /// not running), with whether they carry the neural stage and its
    /// handoff. Retried every tick, so a model is never loaded twice.
    pending: Vec<(Box<dsp::Chain>, bool, usize)>,
    /// When a smaller handoff became possible; the chain is only
    /// rebuilt for a smaller handoff after it has been stable for a while,
    /// because PipeWire changes the quantum as clients come and go.
    shrink_since: Option<std::time::Instant>,
    /// Missed-frame count and time at the start of the current watch window.
    miss_watch: Option<(u64, std::time::Instant)>,
    download: Option<denoise_neural::model::Download>,
}

/// Noise removal is switched off when the denoiser misses more than this
/// many frames (10 ms each) within `MISS_WINDOW`: it has stopped keeping up
/// or its worker died, and the output would only be dry crossfades.
const MAX_MISSES: u64 = 50;
const MISS_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

const SHRINK_AFTER: std::time::Duration = std::time::Duration::from_secs(10);

type LoadResult = Result<Box<dsp::Chain>, String>;

impl Controller {
    /// `engine` must come from `engine::new_engine()` (no neural stage);
    /// the controller adds the neural denoiser when the state asks for it.
    pub fn new(engine: EngineHandle) -> Self {
        let mut c = Self {
            engine,
            profile: VoiceProfile::default(),
            chain_has_neural: false,
            io_latency_ms: 0.0,
            calibrator: None,
            chain_handoff: 0,
            loading: None,
            monitor_has_neural: false,
            monitor_handoff: 0,
            monitor_loading: None,
            io_monitor_ms: 0.0,
            pending: Vec::new(),
            shrink_since: None,
            miss_watch: None,
            download: None,
            state: UiState {
                on: true,
                muted: false,
                ab_original: false,
                preset: PresetId::Broadcast,
                intensity: 60,
                // Without the model the preset starts with noise removal
                // off; choosing a level asks for the download.
                neural_denoise: builtin(PresetId::Broadcast)
                    .default_denoise
                    .filter(|_| denoise_neural::model::present()),
                monitor: MonitorConfig::default(),
                module_enabled: [true; 9],
                meters: MeterFrame::default(),
                mic_latency_ms: 0.0,
                monitor_latency_ms: 0.0,
                input_device: String::new(),
                output_device: String::new(),
                virtual_device: String::new(),
                chain: ChainSettings::default(),
                voice_profile: String::new(),
                cpu_percent: 0.0,
                calibration: None,
                notice: None,
                model: if denoise_neural::model::present() {
                    ModelState::Ready
                } else {
                    ModelState::Missing
                },
                model_prompt: None,
            },
        };
        c.sync_chain();
        c.push_settings();
        c
    }

    /// Sets the backend's own device latency, added to the displayed mic
    /// latency.
    pub fn set_io_latency_ms(&mut self, ms: f32) {
        self.io_latency_ms = ms;
    }

    /// Sets the backend's device latency for the headphone path, ms.
    pub fn set_io_monitor_latency_ms(&mut self, ms: f32) {
        self.io_monitor_ms = ms;
    }

    pub fn set_profile(&mut self, profile: VoiceProfile) {
        self.profile = profile;
        self.push_settings();
    }

    /// Mutable access for the host to fill device names, latencies and
    /// other facts that come from the backend rather than the user.
    pub fn state_mut(&mut self) -> &mut UiState {
        &mut self.state
    }

    pub fn state(&self) -> &UiState {
        &self.state
    }

    /// Starts the 15 s calibration (5 s silence, 10 s speech) on the raw
    /// input. While it runs the input scope queue feeds the calibrator, not
    /// the spectrum analyzer.
    pub fn start_calibration(&mut self) {
        // Drop stale input so calibration starts from now.
        let n = self.engine.scope_in_rx.slots();
        if let Ok(c) = self.engine.scope_in_rx.read_chunk(n) {
            c.commit_all();
        }
        self.calibrator = Some(Calibrator::new());
        self.state.notice = None;
        self.state.calibration = Some(CalibrationStatus {
            phase: Phase::Silence,
            seconds_left: SILENCE_SECONDS as f32,
            message: None,
            error: false,
        });
    }

    pub fn is_calibrating(&self) -> bool {
        self.calibrator.is_some()
    }

    fn pump_calibration(&mut self) {
        let Some(cal) = self.calibrator.as_mut() else {
            return;
        };
        let n = self.engine.scope_in_rx.slots();
        if let Ok(chunk) = self.engine.scope_in_rx.read_chunk(n) {
            let (a, b) = chunk.as_slices();
            cal.feed(a);
            cal.feed(b);
            chunk.commit_all();
        }
        let phase = cal.phase();
        let total = match phase {
            Phase::Silence => SILENCE_SECONDS,
            _ => SPEECH_SECONDS,
        } as f32;
        let mut status = CalibrationStatus {
            phase,
            seconds_left: total * (1.0 - cal.progress()),
            message: None,
            error: false,
        };
        if phase == Phase::Done {
            match cal.result().cloned() {
                Some(Ok(profile)) => {
                    status.message = Some("Calibrated".into());
                    self.state.voice_profile = "Calibrated".into();
                    self.profile = profile;
                    self.push_settings();
                }
                Some(Err(e)) => {
                    status.message = Some(e.to_string());
                    status.error = true;
                    // The standard view has no room for the reason on the
                    // button; the footer shows it as a notice.
                    self.state.notice = Some(format!("Calibration: {e}"));
                }
                None => {}
            }
            self.calibrator = None;
        }
        self.state.calibration = Some(status);
    }

    /// Pulls the latest meters from the engine. Call once per UI frame.
    pub fn tick(&mut self) {
        self.pump_calibration();
        self.poll_download();
        self.retry_pending();
        self.poll_loading();
        // Follow host block changes (the backend may report it after start).
        let wanted = handoff_samples(self.engine.host_block(), dsp::BLOCK_MAIN);
        if self.chain_has_neural
            && self.loading.is_none()
            && self.handoff_needs_rebuild(wanted, self.chain_handoff)
        {
            self.chain_handoff = 0;
            self.sync_chain();
        }
        if let Some(m) = self.engine.latest_meters() {
            self.state.meters = m;
            self.watch_denoiser(m.denoise_missed_frames);
        }
        self.state.mic_latency_ms = self.engine.main_latency_samples() as f32 * 1000.0
            / dsp::SAMPLE_RATE as f32
            + self.io_latency_ms;
        self.state.monitor_latency_ms = self.engine.monitor_latency_samples() as f32 * 1000.0
            / dsp::SAMPLE_RATE as f32
            + self.io_monitor_ms;
        self.poll_monitor_loading();
        let wanted = handoff_samples(self.engine.host_block(), dsp::BLOCK_MAIN);
        if self.monitor_has_neural
            && self.monitor_loading.is_none()
            && wanted > self.monitor_handoff
        {
            self.monitor_handoff = 0;
            self.sync_monitor_chain();
        }
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.state.muted = muted;
        self.push_settings();
    }

    pub fn set_on(&mut self, on: bool) {
        self.state.on = on;
        self.push_settings();
    }

    pub fn set_ab_original(&mut self, original: bool) {
        self.state.ab_original = original;
        self.push_settings();
    }

    /// Selects a preset and applies its default noise removal.
    /// Without the model, a preset with noise removal applies with it off
    /// and asks for the download.
    pub fn set_preset(&mut self, preset: PresetId) {
        self.state.preset = preset;
        let level = builtin(preset).default_denoise;
        self.state.neural_denoise = match level {
            Some(l) if !self.model_ready() => {
                self.state.model_prompt = Some(ModelUse::Noise(l));
                None
            }
            _ => level,
        };
        self.sync_chain();
        self.push_settings();
    }

    /// Turns neural noise removal on at a level, or off with `None`.
    /// The engine rebuilds the main chain without the neural stage when it
    /// is off, removing its 20 ms of latency. Without the model, a level
    /// only asks for the download and is applied once it is ready.
    pub fn set_neural_denoise(&mut self, level: Option<DenoiseLevel>) {
        self.state.notice = None;
        if let Some(l) = level
            && !self.model_ready()
        {
            self.state.model_prompt = Some(ModelUse::Noise(l));
            return;
        }
        self.state.neural_denoise = level;
        self.sync_chain();
        self.push_settings();
    }

    pub fn model_ready(&self) -> bool {
        self.state.model == ModelState::Ready
    }

    /// Starts downloading the model, unless it is there or on its way.
    pub fn download_model(&mut self) {
        if matches!(
            self.state.model,
            ModelState::Ready | ModelState::Downloading(_)
        ) {
            return;
        }
        match denoise_neural::model::Download::start() {
            Ok(d) => {
                self.download = Some(d);
                self.state.model = ModelState::Downloading(0);
            }
            Err(e) => self.state.model = ModelState::Failed(e),
        }
    }

    /// Closes the download dialog, cancelling a download in progress.
    pub fn dismiss_model_prompt(&mut self) {
        self.state.model_prompt = None;
        if self.download.take().is_some() {
            self.state.model = ModelState::Missing;
        }
    }

    fn poll_download(&mut self) {
        let Some(d) = &self.download else {
            return;
        };
        let Some(result) = d.poll() else {
            self.state.model = ModelState::Downloading(d.bytes());
            return;
        };
        self.download = None;
        match result {
            Ok(_) => {
                self.state.model = ModelState::Ready;
                match self.state.model_prompt.take() {
                    Some(ModelUse::Noise(l)) => self.set_neural_denoise(Some(l)),
                    Some(ModelUse::MonitorDenoise) => {
                        let mut m = self.state.monitor;
                        m.with_denoise = true;
                        self.set_monitor(m);
                    }
                    None => {}
                }
            }
            Err(e) => self.state.model = ModelState::Failed(e),
        }
    }

    /// After a load failed: the model may be gone or damaged (and then
    /// removed), so it has to be downloaded again.
    fn recheck_model(&mut self) {
        if !denoise_neural::model::present() {
            self.state.model = ModelState::Missing;
        }
    }

    /// Adds or removes the neural stage when the wanted state differs from
    /// the running chain, or when the host block size calls for a different
    /// handoff. Removing is immediate. Adding loads the model on a
    /// background thread (it takes a while and must not stall the UI or the
    /// calibration feed); [`Controller::tick`] swaps the chain in when ready.
    fn sync_chain(&mut self) {
        let want = self.state.neural_denoise.is_some();
        let handoff = handoff_samples(self.engine.host_block(), dsp::BLOCK_MAIN);
        if !want {
            self.loading = None;
            if self.chain_has_neural {
                let chain = dsp::Chain::new(dsp::PathKind::Main, dsp::BLOCK_MAIN);
                self.submit_chain(Box::new(chain), false, 0);
            }
            return;
        }
        let up_to_date = self.chain_has_neural && self.chain_handoff == handoff;
        let loading_same = self.loading.as_ref().is_some_and(|(_, d)| *d == handoff);
        if up_to_date || loading_same {
            return;
        }
        self.loading = Some((spawn_loader(dsp::PathKind::Main, handoff), handoff));
    }

    /// Swaps in a chain whose model finished loading.
    fn poll_loading(&mut self) {
        let Some((rx, handoff)) = self.loading.as_ref() else {
            return;
        };
        let handoff = *handoff;
        match rx.try_recv() {
            Ok(Ok(chain)) => {
                self.loading = None;
                self.submit_chain(chain, true, handoff);
            }
            Ok(Err(e)) => {
                eprintln!("voice-strip: {e}; noise removal disabled");
                self.loading = None;
                self.state.neural_denoise = None;
                self.recheck_model();
                self.push_settings();
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.loading = None;
                self.state.neural_denoise = None;
                self.push_settings();
            }
        }
    }

    pub fn set_intensity(&mut self, intensity: u8) {
        self.state.intensity = intensity.min(100);
        self.push_settings();
    }

    pub fn set_module_enabled(&mut self, module: Module, enabled: bool) {
        self.state.module_enabled[module.index()] = enabled;
        self.push_settings();
    }

    /// Without the model, noise removal in the headphones asks for the
    /// download and stays off until it is ready.
    pub fn set_monitor(&mut self, mut monitor: MonitorConfig) {
        if monitor.with_denoise && !self.state.monitor.with_denoise && !self.model_ready() {
            monitor.with_denoise = false;
            self.state.model_prompt = Some(ModelUse::MonitorDenoise);
        }
        self.state.monitor = monitor;
        self.engine.set_monitor(monitor);
        self.sync_monitor_chain();
    }

    /// Adds or removes neural denoise in the headphone path (handoff: +20 ms
    /// or more, with a warning in the UI). The model loads in the background.
    fn sync_monitor_chain(&mut self) {
        let want = self.state.monitor.enabled && self.state.monitor.with_denoise;
        let handoff = handoff_samples(self.engine.host_block(), dsp::BLOCK_MAIN);
        if !want {
            self.monitor_loading = None;
            if self.monitor_has_neural {
                let chain = dsp::Chain::new(dsp::PathKind::Monitor, dsp::BLOCK_MONITOR);
                self.submit_chain(Box::new(chain), false, 0);
            }
            return;
        }
        let up_to_date = self.monitor_has_neural && self.monitor_handoff == handoff;
        let loading_same = self
            .monitor_loading
            .as_ref()
            .is_some_and(|(_, d)| *d == handoff);
        if !(up_to_date || loading_same) {
            self.monitor_loading = Some((spawn_loader(dsp::PathKind::Monitor, handoff), handoff));
        }
    }

    fn poll_monitor_loading(&mut self) {
        let Some((rx, handoff)) = self.monitor_loading.as_ref() else {
            return;
        };
        let handoff = *handoff;
        match rx.try_recv() {
            Ok(Ok(chain)) => {
                self.monitor_loading = None;
                self.submit_chain(chain, true, handoff);
            }
            Ok(Err(e)) => {
                eprintln!("voice-strip: {e}; denoise in headphones disabled");
                self.monitor_loading = None;
                self.state.monitor.with_denoise = false;
                self.recheck_model();
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.monitor_loading = None;
                self.state.monitor.with_denoise = false;
            }
        }
    }

    pub fn preset_description(&self) -> &'static str {
        builtin(self.state.preset).description
    }

    pub fn engine_mut(&mut self) -> &mut EngineHandle {
        &mut self.engine
    }

    fn push_settings(&mut self) {
        let s = ChainSettings {
            modules: settings_for(
                self.state.preset,
                self.state.intensity,
                self.state.neural_denoise,
                &self.profile,
            ),
            enabled: self.state.module_enabled,
            bypass: !self.state.on || self.state.ab_original,
            mute: self.state.muted,
        };
        self.state.chain = s;
        self.engine.set_settings(s);
    }
}

/// Loads the neural model on a background thread and builds a chain for
/// `path` around it.
fn spawn_loader(path: dsp::PathKind, handoff: usize) -> std::sync::mpsc::Receiver<LoadResult> {
    let (tx, rx) = std::sync::mpsc::channel();
    let max_block = match path {
        dsp::PathKind::Main => dsp::BLOCK_MAIN,
        dsp::PathKind::Monitor => dsp::BLOCK_MONITOR,
    };
    std::thread::Builder::new()
        .name("dfn-load".into())
        .spawn(move || {
            let result = denoise_neural::DfnDenoiser::spawn()
                .map(|d| {
                    Box::new(dsp::Chain::with_denoiser_handoff(
                        path,
                        max_block,
                        Some(Box::new(d)),
                        handoff,
                    ))
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        })
        .expect("spawn model loader");
    rx
}

impl Controller {
    /// Hands a chain to the engine, or keeps it for a retry on the next tick.
    fn submit_chain(&mut self, chain: Box<dsp::Chain>, neural: bool, handoff: usize) {
        let monitor = chain.path() == dsp::PathKind::Monitor;
        match self.engine.replace_main_chain(chain) {
            Ok(()) => self.chain_accepted(monitor, neural, handoff),
            Err(chain) => {
                // A newer chain for the same path supersedes an older one.
                self.pending.retain(|(c, _, _)| c.path() != chain.path());
                self.pending.push((chain, neural, handoff));
            }
        }
    }

    fn chain_accepted(&mut self, monitor: bool, neural: bool, handoff: usize) {
        if monitor {
            self.monitor_has_neural = neural;
            self.monitor_handoff = handoff;
        } else {
            self.chain_has_neural = neural;
            self.chain_handoff = handoff;
        }
    }

    fn retry_pending(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        for (chain, neural, handoff) in pending {
            self.submit_chain(chain, neural, handoff);
        }
    }

    /// Whether the running chain should be rebuilt for the current host
    /// block: always when more handoff is needed, after `SHRINK_AFTER` of
    /// stability when less would do.
    fn handoff_needs_rebuild(&mut self, wanted: usize, current: usize) -> bool {
        if wanted > current {
            self.shrink_since = None;
            return true;
        }
        if wanted < current {
            let since = *self
                .shrink_since
                .get_or_insert_with(std::time::Instant::now);
            if since.elapsed() >= SHRINK_AFTER {
                self.shrink_since = None;
                return true;
            }
            return false;
        }
        self.shrink_since = None;
        false
    }
}

impl Controller {
    fn watch_denoiser(&mut self, misses: u64) {
        if !self.chain_has_neural || self.loading.is_some() {
            self.miss_watch = None;
            return;
        }
        let now = std::time::Instant::now();
        let (start, since) = *self.miss_watch.get_or_insert((misses, now));
        if misses < start {
            // A new chain restarted the count.
            self.miss_watch = Some((misses, now));
        } else if misses - start > MAX_MISSES {
            self.miss_watch = None;
            self.state.notice =
                Some("Noise removal stopped keeping up and was switched off".into());
            self.state.neural_denoise = None;
            self.sync_chain();
            self.push_settings();
        } else if now.duration_since(since) >= MISS_WINDOW {
            self.miss_watch = Some((misses, now));
        }
    }
}
