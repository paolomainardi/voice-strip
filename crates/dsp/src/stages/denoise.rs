//! Stage 2: noise reduction. An optional neural denoiser (DeepFilterNet,
//! provided by another crate through [`FrameDenoiser`]) with wet/dry mix,
//! followed by a downward expander for pauses.
//!
//! The expander threshold follows the room. While the voice detector (see
//! [`crate::voice`]) hears no speech, the stage measures the noise at the
//! expander input and puts the threshold a margin above it. The detector
//! answers late, so a 10 ms segment counts as room only once the detector
//! has reported silence for [`QUIET_SEGMENTS`] segments after it. Without a
//! detector, or before the room is known, the fixed threshold applies.
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

/// Room measurement segment, samples (10 ms).
const SEGMENT: usize = 480;
/// Segments a measurement waits before it counts (100 ms), so the detector
/// has caught up with the audio it covers.
const ROOM_DELAY: usize = 10;
/// Segments of continuous silence from the detector before the delayed
/// segment counts as room (200 ms): covers the detector's own delay.
pub const QUIET_SEGMENTS: u32 = 20;
/// Room tracking per counted segment when the level rises: a 1 s average.
const ROOM_RISE_TRACK: f32 = 0.01;
/// Room tracking when the level falls: 200 ms, so speech the detector
/// missed is forgotten in the next pause.
const ROOM_FALL_TRACK: f32 = 0.05;
/// Most a segment may count above the room estimate (power ratio, +3 dB):
/// speech the detector misses cannot drag the room up, while a room that
/// really gets louder is still followed, a few dB per second.
const ROOM_RISE: f32 = 2.0;
/// Segments below this mean power (-100 dBFS) are digital silence (a
/// muted interface, a stream starting), not a room: they are not counted.
const ROOM_MIN_POW: f32 = 1e-10;
/// Threshold above the measured room before the preset's margin, dB. The
/// expander's 10 ms detector reads steady noise about 3 dB above its RMS,
/// and short windows of it swing a few dB more: with less headroom, room
/// noise would sit at the threshold and barely be turned down.
const ROOM_HEADROOM_DB: f32 = 6.0;
/// Highest threshold the room may set, dBFS, so loud rooms never gate
/// quiet speech.
const MAX_ROOM_THRESHOLD_DB: f32 = -30.0;

/// Room noise measured while nobody speaks.
struct Room {
    acc: f32,
    n: usize,
    delayed: [f32; ROOM_DELAY],
    pos: usize,
    quiet: u32,
    /// Mean power, 0 while unknown.
    pow: f32,
}

impl Room {
    fn new() -> Self {
        Self {
            acc: 0.0,
            n: 0,
            delayed: [0.0; ROOM_DELAY],
            pos: 0,
            quiet: 0,
            pow: 0.0,
        }
    }

    /// Adds one sample's power; at the end of a segment, counts the delayed
    /// one if the detector has been silent long enough. Returns true when
    /// the room estimate changed.
    #[inline]
    fn tick(&mut self, p: f32, speech: Option<bool>) -> bool {
        self.acc += p;
        self.n += 1;
        if self.n < SEGMENT {
            return false;
        }
        let seg = self.acc / SEGMENT as f32;
        self.acc = 0.0;
        self.n = 0;
        let old = std::mem::replace(&mut self.delayed[self.pos], seg);
        self.pos = (self.pos + 1) % ROOM_DELAY;
        if speech == Some(false) && seg >= ROOM_MIN_POW {
            self.quiet = self.quiet.saturating_add(1);
        } else {
            self.quiet = 0;
        }
        if self.quiet < QUIET_SEGMENTS {
            return false;
        }
        self.pow = if self.pow == 0.0 {
            old
        } else if old < self.pow {
            self.pow + (old - self.pow) * ROOM_FALL_TRACK
        } else {
            self.pow + (old.min(self.pow * ROOM_RISE) - self.pow) * ROOM_RISE_TRACK
        };
        self.pow = crate::util::ftz(self.pow);
        true
    }

    fn db(&self) -> Option<f32> {
        (self.pow > 0.0).then(|| pow_to_db(self.pow))
    }
}

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
    /// Average model run time per frame, ms, `None` before the first.
    fn inference_ms(&self) -> Option<f32> {
        None
    }
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
    /// Fixed threshold from the settings.
    fixed_threshold_db: f32,
    margin_db: Option<f32>,
    /// Threshold in use: the room's when known, else the fixed one.
    threshold_db: f32,
    room: Room,
    speech: Option<bool>,
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
            fixed_threshold_db: -120.0,
            margin_db: None,
            threshold_db: -120.0,
            room: Room::new(),
            speech: None,
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
        self.fixed_threshold_db = s.denoise_expander_threshold_db;
        self.margin_db = s.denoise_expander_margin_db;
        self.update_threshold();
        if let Some(n) = &mut self.neural {
            n.mix.set(s.denoise_mix.clamp(0.0, 1.0));
            if s.denoise_atten_limit_db != n.atten_db {
                n.atten_db = s.denoise_atten_limit_db;
                n.model.set_attenuation_limit_db(n.atten_db);
            }
        }
    }

    /// Speech from the voice detector for the coming block, `None` without
    /// a detector.
    pub fn set_speech(&mut self, speech: Option<bool>) {
        self.speech = speech;
    }

    /// Room noise at the expander input while nobody speaks, dBFS RMS.
    pub fn room_db(&self) -> Option<f32> {
        self.room.db()
    }

    /// Starts from a room measured elsewhere (a previous chain), so a chain
    /// swap does not forget the room.
    pub fn seed_room(&mut self, db: Option<f32>) {
        if let Some(db) = db {
            self.room.pow = db_to_lin(db) * db_to_lin(db);
            self.update_threshold();
        }
    }

    /// Expander threshold in use, dBFS.
    pub fn threshold_db(&self) -> f32 {
        self.threshold_db
    }

    fn update_threshold(&mut self) {
        self.threshold_db = match (self.margin_db, self.room.db()) {
            (Some(m), Some(r)) => (r + ROOM_HEADROOM_DB + m).min(MAX_ROOM_THRESHOLD_DB),
            _ => self.fixed_threshold_db,
        };
    }

    /// Average neural model run time per frame, ms; `None` without one.
    pub fn inference_ms(&self) -> Option<f32> {
        self.neural.as_ref().and_then(|n| n.model.inference_ms())
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
        // The room estimate survives: the room did not change.
        self.room = Room {
            pow: self.room.pow,
            ..Room::new()
        };
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
            if self.room.tick(y * y, self.speech) && self.margin_db.is_some() {
                self.update_threshold();
            }
            *x = self.expand(y);
        }
    }

    fn gain_reduction_db(&self) -> f32 {
        self.gr
    }
}
