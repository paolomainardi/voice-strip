//! Stage 6: opto-style compressor. Feed-forward RMS detector, soft knee,
//! program-dependent release (a fast and a slow release stage, the slow one
//! charged by sustained gain reduction), parallel mix. Zero latency.

use crate::Stage;
use crate::settings::ModuleSettings;
use crate::util::{Smoother, coef_ms, db_to_lin, pow_to_db};

const KNEE_DB: f32 = 6.0;

pub struct Compressor {
    threshold: Smoother,
    ratio: Smoother,
    mix: Smoother,
    det: f32,
    det_coef: f32,
    fast: f32,
    slow: f32,
    att_fast: f32,
    rel_fast: f32,
    att_slow: f32,
    rel_slow: f32,
    gr_db: f32,
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Compressor {
    pub fn new() -> Self {
        Self {
            threshold: Smoother::new(0.0, 50.0),
            ratio: Smoother::new(1.0, 50.0),
            mix: Smoother::new(1.0, 50.0),
            det: 0.0,
            det_coef: coef_ms(5.0),
            fast: 0.0,
            slow: 0.0,
            att_fast: coef_ms(8.0),
            rel_fast: coef_ms(70.0),
            att_slow: coef_ms(400.0),
            rel_slow: coef_ms(1500.0),
            gr_db: 0.0,
        }
    }

    pub fn set(&mut self, s: &ModuleSettings) {
        self.threshold.set(s.comp_threshold_db);
        self.ratio.set(s.comp_ratio.max(1.0));
        self.mix.set(s.comp_parallel_mix.clamp(0.0, 1.0));
    }

    /// Static curve: gain reduction in dB (>= 0) for a detector level.
    pub fn curve(level_db: f32, threshold_db: f32, ratio: f32) -> f32 {
        let slope = 1.0 - 1.0 / ratio;
        let over = level_db - threshold_db;
        if 2.0 * over <= -KNEE_DB {
            0.0
        } else if 2.0 * over.abs() < KNEE_DB {
            slope * (over + KNEE_DB / 2.0).powi(2) / (2.0 * KNEE_DB)
        } else {
            slope * over
        }
    }
}

impl Stage for Compressor {
    fn latency_samples(&self) -> usize {
        0
    }

    fn reset(&mut self) {
        self.det = 0.0;
        self.fast = 0.0;
        self.slow = 0.0;
        self.gr_db = 0.0;
    }

    fn process(&mut self, buf: &mut [f32]) {
        for x in buf.iter_mut() {
            let thr = self.threshold.tick();
            let ratio = self.ratio.tick();
            let mix = self.mix.tick();
            self.det = crate::util::ftz(self.det + (*x * *x - self.det) * self.det_coef);
            if ratio <= 1.0 && self.fast == 0.0 && self.slow == 0.0 {
                continue;
            }
            // RMS detector: mean square to dB, +3 dB to read sine peaks.
            let target = Self::curve(pow_to_db(self.det) + 3.0, thr, ratio);
            let c = if target > self.fast {
                self.att_fast
            } else {
                self.rel_fast
            };
            self.fast += (target - self.fast) * c;
            let st = 0.5 * target;
            let c = if st > self.slow {
                self.att_slow
            } else {
                self.rel_slow
            };
            self.slow += (st - self.slow) * c;
            if self.fast < 1e-6 && self.slow < 1e-6 && target == 0.0 {
                self.fast = 0.0;
                self.slow = 0.0;
            }
            let gr = self.fast.max(self.slow);
            self.gr_db = gr;
            let g = db_to_lin(-gr);
            *x *= mix * g + (1.0 - mix);
        }
        if self.fast == 0.0 && self.slow == 0.0 {
            self.gr_db = 0.0;
        }
    }

    fn gain_reduction_db(&self) -> f32 {
        // Effective reduction including the parallel mix.
        let g = db_to_lin(-self.gr_db);
        let m = self.mix.value();
        -crate::util::lin_to_db(m * g + (1.0 - m))
    }
}
