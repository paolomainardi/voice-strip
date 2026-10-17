//! Stage 9: true-peak lookahead limiter.
//!
//! Gain computation: a 4x interpolated peak detector gives, per sample, the
//! gain needed to keep that region under the ceiling. A running minimum over
//! `HOLD` samples followed by a one-pole release and a `RAMP`-long moving
//! average produces a smooth gain that is guaranteed to be at or below the
//! required gain for every detector value touching the delayed sample:
//!
//! * detector value at time n covers input samples n-S+1..=n (S = FIR
//!   support per phase, 24);
//! * the output at time t is x[t-D] * G[t], G = mean(r[t-RAMP+1..=t]);
//! * r <= running min of g over HOLD samples, so G[t] <= g[n] for every n
//!   with t-D <= n <= t-D+S-1 provided D >= RAMP + S - 2 and HOLD >= D + 1.
//!
//! That bound holds on the sample grid. The true peak of the output also
//! depends on how fast G moves between samples (gain modulation widens the
//! spectrum) and on the reference meter's own interpolator, which reads up
//! to about 0.65 dB high at 16 kHz (measured). `MARGIN_DB` covers both; the test
//! checks white noise, high-frequency sines at several phases and burst
//! trains against the independent ebur128 meter.
//!
//! Latency: `DELAY` samples (1.5 ms).

use crate::Stage;
use crate::settings::ModuleSettings;
use crate::util::{Delay, Smoother, TruePeak, coef_ms, db_to_lin, lin_to_db};

const RAMP: usize = 48;
const DELAY: usize = RAMP + TruePeak::SUPPORT - 1;
const HOLD: usize = DELAY + 1;
/// Safety margin below the ceiling for the intersample error of the
/// interpolator and the output gain modulation.
const MARGIN_DB: f32 = 1.0;

pub struct Limiter {
    tp: TruePeak,
    audio: Delay,
    need: [f32; HOLD],
    need_pos: usize,
    ramp: [f32; RAMP],
    ramp_pos: usize,
    ramp_sum: f64,
    rel: f32,
    rel_coef: f32,
    ceiling: Smoother,
    gain: f32,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new()
    }
}

impl Limiter {
    pub fn new() -> Self {
        Self {
            tp: TruePeak::new(),
            audio: Delay::new(DELAY),
            need: [1.0; HOLD],
            need_pos: 0,
            ramp: [1.0; RAMP],
            ramp_pos: 0,
            ramp_sum: RAMP as f64,
            rel: 1.0,
            rel_coef: coef_ms(80.0),
            ceiling: Smoother::new(db_to_lin(-1.0 - MARGIN_DB), 20.0),
            gain: 1.0,
        }
    }

    pub fn set(&mut self, s: &ModuleSettings) {
        self.ceiling
            .set(db_to_lin(s.limiter_ceiling_dbtp - MARGIN_DB));
    }
}

impl Stage for Limiter {
    fn latency_samples(&self) -> usize {
        DELAY
    }

    fn reset(&mut self) {
        self.tp.reset();
        self.audio.reset();
        self.need = [1.0; HOLD];
        self.ramp = [1.0; RAMP];
        self.ramp_sum = RAMP as f64;
        self.rel = 1.0;
        self.gain = 1.0;
    }

    fn process(&mut self, buf: &mut [f32]) {
        for x in buf.iter_mut() {
            let ceil = self.ceiling.tick();
            let peak = self.tp.tick(*x).max(x.abs());
            let need = if peak > ceil { ceil / peak } else { 1.0 };
            self.need[self.need_pos] = need;
            self.need_pos = (self.need_pos + 1) % HOLD;
            let held = self.need.iter().fold(1.0f32, |a, &b| a.min(b));
            let r = self.rel + (1.0 - self.rel) * self.rel_coef;
            // Snap to unity so the ramp sum returns exactly to RAMP.
            self.rel = held.min(if 1.0 - r < 1e-5 { 1.0 } else { r });
            let old = self.ramp[self.ramp_pos];
            self.ramp[self.ramp_pos] = self.rel;
            self.ramp_pos = (self.ramp_pos + 1) % RAMP;
            self.ramp_sum += self.rel as f64 - old as f64;
            let g = (self.ramp_sum / RAMP as f64) as f32;
            // Exactly unity when no reduction is pending, so the stage nulls.
            self.gain = if self.ramp_sum >= RAMP as f64 - 1e-9 {
                1.0
            } else {
                g.min(1.0)
            };
            *x = self.audio.tick(*x) * self.gain;
        }
    }

    fn gain_reduction_db(&self) -> f32 {
        -lin_to_db(self.gain)
    }
}
