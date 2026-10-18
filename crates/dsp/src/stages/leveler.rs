//! Stage 8: slow AGC toward a loudness target.
//!
//! Measures K-weighted loudness of its own input in 100 ms sub-blocks,
//! gates out pauses (absolute -50 LUFS and 10 LU below the tracked level,
//! the EBU R128 gates, so it agrees with the loudness the target is defined
//! in: a wider gate counts room noise around speech and overshoots), tracks
//! a 1 s exponential average in the power domain (so it follows integrated
//! loudness, not the mean of dB values), and slews its gain
//! toward `target - tracked` at a bounded rate. A slow integral correction
//! driven by the chain's final output (fed back through
//! [`Leveler::observe_output`]) removes the residual error left by later
//! stages, such as the limiter shaving loudness off peaks. Zero latency.
//!
//! With a voice detector (see [`crate::voice`]), a 400 ms window in which
//! it heard no speech at all is ignored on both sides, so steady noise
//! loud enough to pass the gates never moves the gain.

use crate::Stage;
use crate::loudness::KWeighting;
use crate::settings::ModuleSettings;
use crate::util::db_to_lin;

const SUB: usize = 4_800;
const TRACK: f64 = 0.15;
const ABS_GATE_LUFS: f64 = -50.0;
const REL_GATE_LU: f64 = 10.0;
const MAX_UP_DB: f32 = 24.0;
/// Integral gain of the output correction per 100 ms sub-block.
const KI: f64 = 0.04;
/// Positive correction is capped low so the loop cannot push ever harder
/// into the limiter, which removes loudness as gain rises.
const MAX_CORRECTION_UP_DB: f64 = 8.0;
const MAX_CORRECTION_DOWN_DB: f64 = -8.0;
/// Sub-blocks of continuous relative gating (about 3 s) after which the
/// tracked level is re-seeded, so a lasting level drop is followed.
const RESEED_SUBS: u32 = 30;
const MAX_DOWN_DB: f32 = -12.0;
/// Gain slew rate, dB per second.
const SLEW_DB_S: f32 = 6.0;

pub struct Leveler {
    target: Option<f32>,
    kw: KWeighting,
    acc: f64,
    n: usize,
    sub: [f64; 4],
    sub_pos: usize,
    sub_count: usize,
    tracked: Option<f64>,
    gated_subs: u32,
    desired_db: f32,
    out_kw: KWeighting,
    out_acc: f64,
    out_n: usize,
    out_sub: [f64; 4],
    out_pos: usize,
    out_count: usize,
    out_tracked: Option<f64>,
    correction_db: f64,
    gain_db: f32,
    gain_lin: f32,
    slew: f32,
    /// Input tracking coefficient per sub-block (`TRACK` scaled by speed).
    track: f64,
    /// Speech from the voice detector, `None` without one.
    speech: Option<bool>,
    /// Speech heard in the current and last sub-blocks, input side.
    speech_sub: [bool; 4],
    /// Speech heard in the current and last sub-blocks, output side.
    out_speech_sub: [bool; 4],
}

impl Default for Leveler {
    fn default() -> Self {
        Self::new()
    }
}

fn lufs(p: f64) -> f64 {
    -0.691 + 10.0 * p.max(1e-20).log10()
}

impl Leveler {
    pub fn new() -> Self {
        Self {
            target: None,
            kw: KWeighting::new(),
            acc: 0.0,
            n: 0,
            sub: [0.0; 4],
            sub_pos: 0,
            sub_count: 0,
            tracked: None,
            gated_subs: 0,
            desired_db: 0.0,
            out_kw: KWeighting::new(),
            out_acc: 0.0,
            out_n: 0,
            out_sub: [0.0; 4],
            out_pos: 0,
            out_count: 0,
            out_tracked: None,
            correction_db: 0.0,
            gain_db: 0.0,
            gain_lin: 1.0,
            slew: SLEW_DB_S / crate::SAMPLE_RATE as f32,
            track: TRACK,
            speech: None,
            speech_sub: [false; 4],
            out_speech_sub: [false; 4],
        }
    }

    /// Speech from the voice detector for the coming block, `None` without
    /// a detector.
    pub fn set_speech(&mut self, speech: Option<bool>) {
        self.speech = speech;
        if speech == Some(true) {
            self.speech_sub[self.sub_pos] = true;
            self.out_speech_sub[self.out_pos] = true;
        }
    }

    /// Whether the detector, if any, heard speech in the window.
    fn window_has_speech(&self, subs: &[bool; 4]) -> bool {
        self.speech.is_none() || subs.iter().any(|&s| s)
    }

    pub fn set(&mut self, s: &ModuleSettings) {
        self.target = s.leveler_target_lufs;
        let speed = s.leveler_speed.clamp(0.05, 1.0);
        self.track = TRACK * speed as f64;
        self.slew = SLEW_DB_S * speed / crate::SAMPLE_RATE as f32;
        if self.target.is_none() {
            self.desired_db = 0.0;
            self.correction_db = 0.0;
        }
    }

    /// Closed-loop correction currently applied, dB.
    pub fn correction_db(&self) -> f32 {
        self.correction_db as f32
    }

    /// Current applied gain in dB (positive = boost).
    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    fn end_sub_block(&mut self) {
        self.sub[self.sub_pos] = self.acc / SUB as f64;
        self.sub_pos = (self.sub_pos + 1) % 4;
        self.sub_count = (self.sub_count + 1).min(4);
        self.acc = 0.0;
        self.n = 0;
        let speech = self.window_has_speech(&self.speech_sub);
        self.speech_sub[self.sub_pos] = self.speech == Some(true);
        if self.sub_count < 4 {
            return;
        }
        let m = self.sub.iter().sum::<f64>() / 4.0;
        let m_lufs = lufs(m);
        if m_lufs < ABS_GATE_LUFS || !speech {
            return;
        }
        match self.tracked {
            None => self.tracked = Some(m),
            Some(t) => {
                if m_lufs < lufs(t) - REL_GATE_LU {
                    self.gated_subs += 1;
                    if self.gated_subs < RESEED_SUBS {
                        return;
                    }
                    self.tracked = Some(m);
                } else {
                    self.tracked = Some(t + self.track * (m - t));
                }
                self.gated_subs = 0;
            }
        }
        let t = lufs(self.tracked.unwrap_or(m));
        if let Some(target) = self.target {
            self.desired_db = (target as f64 - t + self.correction_db)
                .clamp(MAX_DOWN_DB as f64, MAX_UP_DB as f64) as f32;
        }
    }

    /// Feeds the chain's final output for the block just processed. Main
    /// path only; realtime-safe.
    pub fn observe_output(&mut self, buf: &[f32]) {
        let Some(target) = self.target else {
            return;
        };
        for &x in buf {
            let k = self.out_kw.tick(x) as f64;
            self.out_acc += k * k;
            self.out_n += 1;
            if self.out_n < SUB {
                continue;
            }
            self.out_sub[self.out_pos] = self.out_acc / SUB as f64;
            self.out_pos = (self.out_pos + 1) % 4;
            self.out_count = (self.out_count + 1).min(4);
            self.out_acc = 0.0;
            self.out_n = 0;
            let speech = self.window_has_speech(&self.out_speech_sub);
            self.out_speech_sub[self.out_pos] = self.speech == Some(true);
            if self.out_count < 4 {
                continue;
            }
            let m = self.out_sub.iter().sum::<f64>() / 4.0;
            let m_lufs = lufs(m);
            // Same gating as the input side: ignore pauses.
            let gated = !speech
                || m_lufs < ABS_GATE_LUFS
                || self
                    .out_tracked
                    .is_some_and(|t| m_lufs < lufs(t) - REL_GATE_LU);
            if gated {
                continue;
            }
            let t = match self.out_tracked {
                None => m,
                Some(t) => t + TRACK * (m - t),
            };
            self.out_tracked = Some(t);
            self.correction_db = (self.correction_db + KI * (target as f64 - lufs(t)))
                .clamp(MAX_CORRECTION_DOWN_DB, MAX_CORRECTION_UP_DB);
        }
    }
}

impl Stage for Leveler {
    fn latency_samples(&self) -> usize {
        0
    }

    fn reset(&mut self) {
        self.kw.reset();
        self.acc = 0.0;
        self.n = 0;
        self.sub = [0.0; 4];
        self.sub_pos = 0;
        self.sub_count = 0;
        self.tracked = None;
        self.gated_subs = 0;
        self.desired_db = 0.0;
        self.out_kw.reset();
        self.out_acc = 0.0;
        self.out_n = 0;
        self.out_sub = [0.0; 4];
        self.out_pos = 0;
        self.out_count = 0;
        self.out_tracked = None;
        self.correction_db = 0.0;
        self.gain_db = 0.0;
        self.gain_lin = 1.0;
        self.speech_sub = [false; 4];
        self.out_speech_sub = [false; 4];
    }

    fn process(&mut self, buf: &mut [f32]) {
        for x in buf.iter_mut() {
            let k = self.kw.tick(*x) as f64;
            self.acc += k * k;
            self.n += 1;
            if self.n == SUB {
                self.end_sub_block();
            }
            if self.gain_db != self.desired_db {
                let d = self.desired_db - self.gain_db;
                self.gain_db += d.clamp(-self.slew, self.slew);
                self.gain_lin = db_to_lin(self.gain_db);
            }
            *x *= self.gain_lin;
        }
    }

    fn gain_reduction_db(&self) -> f32 {
        -self.gain_db
    }
}
