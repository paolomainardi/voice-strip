use crate::Stage;
use crate::loudness::LoudnessMeter;
use crate::settings::{ChainSettings, Module};
use crate::stages::{
    compressor::Compressor, deesser::DeEsser, denoise::Denoise, denoise::FrameDenoiser,
    deplosive::DePlosive, dyneq::DynamicEq, input::Input, leveler::Leveler, limiter::Limiter,
    saturation::Saturation,
};
use crate::util::{Delay, Smoother};
use crate::voice::SpeechGate;

/// Which output path a chain instance serves. The monitor chain skips the
/// modules for which [`Module::in_monitor_path`] is false.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathKind {
    Main,
    Monitor,
}

/// Per-block meter values produced by the chain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChainMeters {
    /// Gain reduction per module in dB (positive), indexed by [`Module::index`].
    /// The leveler reports negative values when it boosts.
    pub gain_reduction_db: [f32; 9],
    /// Output loudness, main path only (`NEG_INFINITY` otherwise).
    pub lufs_momentary: f32,
    pub lufs_short_term: f32,
    /// Output true peak over this block, dBTP.
    pub true_peak_db: f32,
    /// Speech probability from the voice detector, `None` without one.
    pub speech_probability: Option<f32>,
    /// Room noise learned while nobody speaks, dBFS RMS at the expander
    /// input, `None` while unknown.
    pub room_db: Option<f32>,
}

impl Default for ChainMeters {
    fn default() -> Self {
        Self {
            gain_reduction_db: [0.0; 9],
            lufs_momentary: f32::NEG_INFINITY,
            lufs_short_term: f32::NEG_INFINITY,
            true_peak_db: f32::NEG_INFINITY,
            speech_probability: None,
            room_db: None,
        }
    }
}

const FADE_MS: f32 = 10.0;

/// Enable/disable crossfade around one stage. The dry signal is delayed by
/// the stage latency so both sides stay aligned and the chain latency does
/// not change when a module is toggled.
struct Slot {
    active: bool,
    mix: Smoother,
    dry_delay: Delay,
}

impl Slot {
    fn new(latency: usize, active: bool) -> Self {
        Self {
            active,
            mix: Smoother::new(1.0, FADE_MS),
            dry_delay: Delay::new(latency),
        }
    }
}

pub struct Chain {
    path: PathKind,
    input: Input,
    denoise: Denoise,
    deplosive: DePlosive,
    deesser: DeEsser,
    dyneq: DynamicEq,
    compressor: Compressor,
    saturation: Saturation,
    leveler: Leveler,
    limiter: Limiter,
    slots: [Slot; 9],
    scratch: Vec<f32>,
    bypass: Smoother,
    /// Output gain: 1 normally, 0 when muted, ramped to avoid clicks.
    mute_gain: Smoother,
    bypass_delay: Delay,
    bypass_scratch: Vec<f32>,
    loudness: LoudnessMeter,
    latency: usize,
    speech_probability: Option<f32>,
    speech_gate: SpeechGate,
}

impl Chain {
    /// Allocates all buffers for blocks up to `max_block` samples.
    pub fn new(path: PathKind, max_block: usize) -> Self {
        Self::with_denoiser(path, max_block, None)
    }

    /// Like [`Chain::new`], with a neural denoiser in the Denoise stage. Its
    /// latency becomes part of the chain latency.
    pub fn with_denoiser(
        path: PathKind,
        max_block: usize,
        neural: Option<Box<dyn FrameDenoiser>>,
    ) -> Self {
        Self::with_denoiser_handoff(path, max_block, neural, 0)
    }

    /// Like [`Chain::with_denoiser`] with an explicit handoff for the
    /// neural stage, samples (`dsp::stages::denoise::handoff_samples`; less
    /// than a frame means one frame).
    pub fn with_denoiser_handoff(
        path: PathKind,
        max_block: usize,
        neural: Option<Box<dyn FrameDenoiser>>,
        handoff: usize,
    ) -> Self {
        let mut c = Self {
            path,
            input: Input::new(),
            denoise: match neural {
                Some(n) => Denoise::with_neural(n, handoff),
                None => Denoise::new(),
            },
            deplosive: DePlosive::new(),
            deesser: DeEsser::new(),
            dyneq: DynamicEq::new(),
            compressor: Compressor::new(),
            saturation: Saturation::new(),
            leveler: Leveler::new(),
            limiter: Limiter::new(),
            slots: std::array::from_fn(|_| Slot::new(0, false)),
            scratch: vec![0.0; max_block],
            bypass: Smoother::new(0.0, FADE_MS),
            mute_gain: Smoother::new(1.0, FADE_MS),
            bypass_delay: Delay::new(1),
            bypass_scratch: vec![0.0; max_block],
            loudness: LoudnessMeter::new(),
            latency: 0,
            speech_probability: None,
            speech_gate: SpeechGate::default(),
        };
        let mut total = 0;
        // The monitor path skips the denoiser unless a neural one is given
        // explicitly (the user's "denoise in headphones" option).
        let monitor_neural = path == PathKind::Monitor && c.denoise.latency_samples() > 0;
        for m in Module::ALL {
            let active = path == PathKind::Main
                || m.in_monitor_path()
                || (m == Module::Denoise && monitor_neural);
            let lat = c.stage(m).latency_samples();
            c.slots[m.index()] = Slot::new(lat, active);
            if active {
                total += lat;
            }
        }
        c.latency = total;
        c.bypass_delay = Delay::new(total);
        c
    }

    pub fn path(&self) -> PathKind {
        self.path
    }

    fn stage(&mut self, m: Module) -> &mut dyn Stage {
        match m {
            Module::Input => &mut self.input,
            Module::Denoise => &mut self.denoise,
            Module::DePlosive => &mut self.deplosive,
            Module::DeEsser => &mut self.deesser,
            Module::DynamicEq => &mut self.dyneq,
            Module::Compressor => &mut self.compressor,
            Module::Saturation => &mut self.saturation,
            Module::Leveler => &mut self.leveler,
            Module::Limiter => &mut self.limiter,
        }
    }

    /// Applies new settings. Realtime-safe; parameter changes are smoothed
    /// inside the stages.
    pub fn set_settings(&mut self, s: &ChainSettings) {
        let m = &s.modules;
        self.input.set(m);
        self.denoise.set(m);
        self.deplosive.set(m);
        self.deesser.set(m);
        self.dyneq.set(m);
        self.compressor.set(m);
        self.saturation.set(m);
        self.leveler.set(m);
        self.limiter.set(m);
        for (slot, &on) in self.slots.iter_mut().zip(&s.enabled) {
            slot.mix.set(if on { 1.0 } else { 0.0 });
        }
        self.bypass.set(if s.bypass { 1.0 } else { 0.0 });
        self.mute_gain.set(if s.mute { 0.0 } else { 1.0 });
    }

    /// Total latency of this path, in samples. Constant: toggling modules
    /// or bypass does not change it.
    pub fn latency_samples(&self) -> usize {
        self.latency
    }

    /// Leveler state for diagnostics: (applied gain, closed-loop correction), dB.
    pub fn leveler_state_db(&self) -> (f32, f32) {
        (self.leveler.gain_db(), self.leveler.correction_db())
    }

    /// Latest speech probability from the voice detector, for the coming
    /// block; `None` without a detector. Realtime-safe.
    pub fn set_speech_probability(&mut self, p: Option<f32>) {
        self.speech_probability = p;
        let speech = p.map(|p| self.speech_gate.update(p));
        self.denoise.set_speech(speech);
        self.leveler.set_speech(speech);
    }

    /// Room noise learned while nobody speaks, dBFS RMS.
    pub fn room_db(&self) -> Option<f32> {
        self.denoise.room_db()
    }

    /// Starts from the room another chain learned. Realtime-safe.
    pub fn seed_room(&mut self, db: Option<f32>) {
        self.denoise.seed_room(db);
    }

    /// Expander threshold in use, dBFS.
    pub fn expander_threshold_db(&self) -> f32 {
        self.denoise.threshold_db()
    }

    /// Average neural denoiser run time per frame, ms; `None` without one.
    pub fn denoise_inference_ms(&self) -> Option<f32> {
        self.denoise.inference_ms()
    }

    /// Frames the neural denoiser failed to deliver in time.
    pub fn denoise_missed_frames(&self) -> u64 {
        self.denoise.missed_frames()
    }

    pub fn reset(&mut self) {
        for m in Module::ALL {
            self.stage(m).reset();
        }
        for s in &mut self.slots {
            s.dry_delay.reset();
        }
        self.bypass_delay.reset();
        self.loudness.reset();
    }

    /// Processes one block in place. `buf.len()` must not exceed `max_block`.
    pub fn process(&mut self, buf: &mut [f32]) -> ChainMeters {
        let n = buf.len();
        debug_assert!(n <= self.scratch.len());
        let mut meters = ChainMeters {
            speech_probability: self.speech_probability,
            ..ChainMeters::default()
        };

        let bypass_active = !(self.bypass.is_settled() && self.bypass.value() == 0.0);
        if bypass_active {
            for (d, &x) in self.bypass_scratch[..n].iter_mut().zip(buf.iter()) {
                *d = self.bypass_delay.tick(x);
            }
        } else if self.latency > 0 {
            // Keep the bypass delay line current so switching is seamless.
            for &x in buf.iter() {
                self.bypass_delay.tick(x);
            }
        }
        let fully_bypassed = self.bypass.is_settled() && self.bypass.value() == 1.0;

        if !fully_bypassed || self.latency > 0 {
            for m in Module::ALL {
                self.run_slot(m, buf);
                meters.gain_reduction_db[m.index()] = if self.slots[m.index()].active {
                    self.stage(m).gain_reduction_db() * self.slots[m.index()].mix.value()
                } else {
                    0.0
                };
            }
        }

        if bypass_active {
            for (x, &d) in buf.iter_mut().zip(&self.bypass_scratch[..n]) {
                let b = self.bypass.tick();
                *x = *x * (1.0 - b) + d * b;
            }
        }

        // Mute last, so the meters below show what others actually hear.
        if !(self.mute_gain.is_settled() && self.mute_gain.value() == 1.0) {
            for x in buf.iter_mut() {
                *x *= self.mute_gain.tick();
            }
        }

        if self.path == PathKind::Main {
            let lev = &self.slots[Module::Leveler.index()];
            // The leveler must not chase the silence of a muted output.
            let muted = self.mute_gain.target() == 0.0;
            if lev.mix.is_settled() && lev.mix.value() == 1.0 && !self.bypass_engaged() && !muted {
                self.leveler.observe_output(buf);
            }
            self.loudness.process(buf);
            meters.lufs_momentary = self.loudness.momentary();
            meters.lufs_short_term = self.loudness.short_term();
            meters.true_peak_db = self.loudness.take_true_peak_db();
            meters.room_db = self.denoise.room_db();
        }
        meters
    }

    fn bypass_engaged(&self) -> bool {
        self.bypass.target() != 0.0
    }

    fn run_slot(&mut self, m: Module, buf: &mut [f32]) {
        let i = m.index();
        if !self.slots[i].active {
            return;
        }
        let n = buf.len();
        let settled = self.slots[i].mix.is_settled();
        let mix = self.slots[i].mix.value();
        let has_latency = self.slots[i].dry_delay.len() > 1 || self.stage(m).latency_samples() > 0;
        if settled && mix == 1.0 && !has_latency {
            self.stage(m).process(buf);
            return;
        }
        if settled && mix == 0.0 && !has_latency {
            // Off and latency-free: skip it entirely; reset so it restarts
            // clean when re-enabled.
            self.stage(m).reset();
            return;
        }
        // Crossfade (or latency-bearing stage): run the stage and blend with
        // the dry signal delayed by the stage latency.
        let mut scratch = std::mem::take(&mut self.scratch);
        for (d, &x) in scratch[..n].iter_mut().zip(buf.iter()) {
            *d = if has_latency {
                self.slots[i].dry_delay.tick(x)
            } else {
                x
            };
        }
        self.stage(m).process(buf);
        let slot = &mut self.slots[i];
        for (x, &d) in buf.iter_mut().zip(&scratch[..n]) {
            let w = slot.mix.tick();
            if w != 1.0 {
                *x = *x * w + d * (1.0 - w);
            }
        }
        self.scratch = scratch;
    }
}
