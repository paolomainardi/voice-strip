//! Stage 2: noise reduction. An optional neural denoiser (DeepFilterNet,
//! provided by another crate through [`FrameDenoiser`]) with wet/dry mix,
//! followed by a downward expander for pauses.
//!
//! The neural model runs off the audio thread. Frames move through
//! lock-free queues: a frame is submitted when complete, and its result is
//! fetched a fixed handoff later, chosen by [`handoff_samples`] from the
//! host block size so the worker always gets at least [`WORKER_BUDGET`]
//! samples of wall time, whatever the callback size. Stage latency is
//! `handoff + model latency`, constant whatever the mix.
//!
//! Frames are counted in flight. A frame the worker did not deliver in time
//! is replaced by the equally delayed dry signal with a short crossfade, and
//! its late result is discarded when it arrives, so one miss never shifts
//! the wet signal against the dry one.

use crate::Stage;
use crate::settings::ModuleSettings;
use crate::util::{Delay, Smoother, coef_ms, db_to_lin, pow_to_db};

const RATIO: f32 = 3.0;
const RANGE_DB: f32 = 15.0;
const HOLD_SAMPLES: u32 = 2_400;
const MAX_FRAME: usize = 1_024;

/// A frame-based denoiser running outside the audio thread. `submit` and
/// `fetch` are called on the audio thread and must be wait-free and
/// allocation-free.
pub trait FrameDenoiser: Send {
    /// Frame length in samples (at most 1024).
    fn frame_len(&self) -> usize;
    /// Delay of the model itself, in samples.
    fn model_latency_samples(&self) -> usize;
    /// Queues one frame. Returns false when the queue is full.
    fn submit(&mut self, frame: &[f32]) -> bool;
    /// Takes the oldest finished frame. Returns false when none is ready.
    fn fetch(&mut self, out: &mut [f32]) -> bool;
    /// Most frames that may be in flight (queue capacity). The stage caps
    /// its handoff below it.
    fn max_in_flight(&self) -> usize {
        usize::MAX
    }
    /// Wet attenuation limit in dB (how much noise the model may remove).
    fn set_attenuation_limit_db(&mut self, _db: f32) {}
}

/// Least wall time the worker gets for one frame, samples (2.67 ms at
/// 48 kHz; inference takes at most 1.6 ms on the measured machines).
pub const WORKER_BUDGET: usize = 128;

/// Handoff, in samples from a frame's last input sample to the first output
/// sample of its result, for chains fed blocks of `host_block` samples
/// (`None`: variable, up to one frame). The chain processes host blocks
/// directly, so a frame can complete anywhere in a callback.
///
/// - Blocks that are whole frames: frames end with the callback; the result
///   is due the next callback, one block later.
/// - Blocks that divide a frame: frames end with a callback; the result is
///   due enough callbacks later to cover the budget.
/// - Other blocks: a frame may end at a callback's last sample, so the
///   result is read one block plus the budget later.
pub fn handoff_samples(host_block: Option<usize>, frame: usize) -> usize {
    match host_block.filter(|&n| n > 0) {
        Some(n) if n.is_multiple_of(frame) => n,
        Some(n) if frame.is_multiple_of(n) => frame + (WORKER_BUDGET.div_ceil(n) - 1) * n,
        Some(n) => frame + n + WORKER_BUDGET,
        None => 2 * frame + WORKER_BUDGET,
    }
}

struct Neural {
    model: Box<dyn FrameDenoiser>,
    frame: usize,
    /// Whole frames of handoff.
    depth: usize,
    /// Samples of handoff beyond `depth` frames, below one frame.
    slack: usize,
    in_flight: usize,
    /// In-flight frames submitted before a reset, to discard.
    stale: usize,
    scratch: [f32; MAX_FRAME],
    latency: usize,
    inbuf: [f32; MAX_FRAME],
    outbuf: [f32; MAX_FRAME],
    out_valid: bool,
    primed: bool,
    pos: usize,
    dry: Delay,
    mix: Smoother,
    valid: Smoother,
    misses: u64,
    atten_db: f32,
}

pub struct Denoise {
    neural: Option<Box<Neural>>,
    threshold_db: f32,
    det: f32,
    det_coef: f32,
    gr: f32,
    att: f32,
    rel: f32,
    hold: u32,
}

impl Default for Denoise {
    fn default() -> Self {
        Self::new()
    }
}

impl Denoise {
    pub fn new() -> Self {
        Self {
            neural: None,
            threshold_db: -120.0,
            det: 0.0,
            det_coef: coef_ms(10.0),
            gr: 0.0,
            att: coef_ms(3.0),
            rel: coef_ms(120.0),
            hold: 0,
        }
    }

    /// Attaches a neural denoiser with a handoff of `handoff` samples (see
    /// [`handoff_samples`]; at least one frame). Call before the chain runs;
    /// it fixes the stage latency.
    pub fn with_neural(model: Box<dyn FrameDenoiser>, handoff: usize) -> Self {
        let frame = model.frame_len();
        assert!(frame > 0 && frame <= MAX_FRAME, "unsupported frame length");
        let depth = (handoff / frame).clamp(1, model.max_in_flight().saturating_sub(1).max(1));
        let slack = handoff % frame;
        let latency = depth * frame + slack + model.model_latency_samples();
        Self {
            neural: Some(Box::new(Neural {
                model,
                frame,
                depth,
                slack,
                in_flight: 0,
                stale: 0,
                scratch: [0.0; MAX_FRAME],
                latency,
                inbuf: [0.0; MAX_FRAME],
                outbuf: [0.0; MAX_FRAME],
                out_valid: false,
                primed: false,
                pos: 0,
                dry: Delay::new(latency),
                mix: Smoother::new(0.0, 30.0),
                valid: Smoother::new(0.0, 2.0),
                misses: 0,
                atten_db: f32::NAN,
            })),
            ..Self::new()
        }
    }

    pub fn set(&mut self, s: &ModuleSettings) {
        self.threshold_db = s.denoise_expander_threshold_db;
        if let Some(n) = &mut self.neural {
            n.mix.set(s.denoise_mix.clamp(0.0, 1.0));
            if s.denoise_atten_limit_db != n.atten_db {
                n.atten_db = s.denoise_atten_limit_db;
                n.model.set_attenuation_limit_db(n.atten_db);
            }
        }
    }

    /// Frames the neural worker failed to deliver in time.
    pub fn missed_frames(&self) -> u64 {
        self.neural.as_ref().map_or(0, |n| n.misses)
    }

    fn expand(&mut self, x: f32) -> f32 {
        self.det = crate::util::ftz(self.det + (x * x - self.det) * self.det_coef);
        let lvl = pow_to_db(self.det) + 3.0;
        let below = self.threshold_db - lvl;
        let target = if below > 0.0 {
            (below * (RATIO - 1.0)).min(RANGE_DB)
        } else {
            0.0
        };
        // Voice reopens fast; closing waits for the hold time so word
        // tails are not cut.
        if target < self.gr {
            self.gr += (target - self.gr) * self.att;
            self.hold = HOLD_SAMPLES;
        } else if self.hold > 0 {
            self.hold -= 1;
        } else {
            self.gr += (target - self.gr) * self.rel;
        }
        if self.gr < 1e-5 && target == 0.0 {
            self.gr = 0.0;
            return x;
        }
        x * db_to_lin(-self.gr)
    }
}

impl Neural {
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        // Results are fetched `slack` samples into a frame period and played
        // from there, so the worker gets the slack on top of the frames.
        if self.pos == self.slack && self.primed {
            // Discard results from before a reset, then late results of
            // frames already replaced by dry.
            while self.stale > 0 && self.model.fetch(&mut self.scratch[..self.frame]) {
                self.stale -= 1;
                self.in_flight -= 1;
            }
            while self.in_flight - self.stale > self.depth
                && self.model.fetch(&mut self.scratch[..self.frame])
            {
                self.in_flight -= 1;
            }
            self.out_valid = self.stale == 0
                && self.in_flight == self.depth
                && self.model.fetch(&mut self.outbuf[..self.frame]);
            if self.out_valid {
                self.in_flight -= 1;
            } else if self.stale == 0 && self.in_flight >= self.depth {
                self.misses += 1;
            }
            self.valid.set(if self.out_valid { 1.0 } else { 0.0 });
        }
        let dry = self.dry.tick(x);
        self.inbuf[self.pos] = x;
        let wet = self.outbuf[(self.pos + self.frame - self.slack) % self.frame];
        self.pos += 1;
        if self.pos == self.frame {
            self.pos = 0;
            if self.model.submit(&self.inbuf[..self.frame]) {
                self.primed = true;
                self.in_flight += 1;
            } else {
                self.misses += 1;
            }
        }
        let w = self.mix.tick() * self.valid.tick();
        if w == 0.0 {
            dry
        } else {
            wet * w + dry * (1.0 - w)
        }
    }
}

impl Stage for Denoise {
    fn latency_samples(&self) -> usize {
        self.neural.as_ref().map_or(0, |n| n.latency)
    }

    fn reset(&mut self) {
        self.det = 0.0;
        self.gr = 0.0;
        self.hold = 0;
        if let Some(n) = &mut self.neural {
            n.dry.reset();
            n.pos = 0;
            n.outbuf = [0.0; MAX_FRAME];
            n.out_valid = false;
            n.valid.snap(0.0);
            // Results still in flight belong to the old timeline.
            n.stale = n.in_flight;
        }
    }

    fn process(&mut self, buf: &mut [f32]) {
        for x in buf.iter_mut() {
            let y = match &mut self.neural {
                Some(n) => n.tick(*x),
                None => *x,
            };
            *x = self.expand(y);
        }
    }

    fn gain_reduction_db(&self) -> f32 {
        self.gr
    }
}
