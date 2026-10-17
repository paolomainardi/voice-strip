//! Stage 3: de-plosive. Compresses the band below 150 Hz only on
//! transients: a fast envelope of the low band is compared with a slow one,
//! and reduction applies to the part of the fast envelope that exceeds both
//! the threshold and the slow envelope by more than 6 dB. Sustained low end
//! (slow envelope tracks it) is left alone. The cut is a dynamic low
//! shelf at the split frequency. Zero latency.

use crate::Stage;
use crate::settings::ModuleSettings;
use crate::util::{Biquad, Coefs, DynamicShelf, coef_ms, pow_to_db};

const SPLIT_HZ: f32 = 150.0;
const TRANSIENT_DB: f32 = 6.0;
const RATIO: f32 = 6.0;
const MAX_GR_DB: f32 = 18.0;
const FREEZE_MAX: u32 = 3_840;

pub struct DePlosive {
    threshold_db: f32,
    split: Biquad,
    shelf: DynamicShelf,
    fast: f32,
    slow: f32,
    fast_coef: f32,
    slow_coef: f32,
    gr: f32,
    frozen: u32,
    att: f32,
    rel: f32,
}

impl Default for DePlosive {
    fn default() -> Self {
        Self::new()
    }
}

impl DePlosive {
    pub fn new() -> Self {
        Self {
            threshold_db: 0.0,
            split: Biquad::new(Coefs::lowpass(SPLIT_HZ, std::f32::consts::FRAC_1_SQRT_2)),
            shelf: DynamicShelf::new(SPLIT_HZ, false),
            fast: 0.0,
            slow: 0.0,
            fast_coef: coef_ms(3.0),
            slow_coef: coef_ms(300.0),
            gr: 0.0,
            frozen: 0,
            att: coef_ms(0.5),
            rel: coef_ms(50.0),
        }
    }

    pub fn set(&mut self, s: &ModuleSettings) {
        self.threshold_db = s.deplosive_threshold_db;
    }
}

impl Stage for DePlosive {
    fn latency_samples(&self) -> usize {
        0
    }

    fn reset(&mut self) {
        self.split.reset();
        self.shelf.reset();
        self.fast = 0.0;
        self.slow = 0.0;
        self.gr = 0.0;
        self.frozen = 0;
    }

    fn process(&mut self, buf: &mut [f32]) {
        let slope = 1.0 - 1.0 / RATIO;
        for x in buf.iter_mut() {
            let low = self.split.tick(*x);
            let p = low * low;
            self.fast = crate::util::ftz(self.fast + (p - self.fast) * self.fast_coef);
            let fast_db = pow_to_db(self.fast) + 3.0;
            let mut slow_db = pow_to_db(self.slow) + 3.0;
            // Freeze the slow envelope during a transient so the burst does
            // not raise its own reference.
            // A transient longer than 80 ms is sustained low end, not a plosive.
            if fast_db < slow_db + TRANSIENT_DB {
                self.frozen = 0;
            } else {
                self.frozen += 1;
            }
            if self.frozen == 0 || self.frozen > FREEZE_MAX {
                self.slow = crate::util::ftz(self.slow + (p - self.slow) * self.slow_coef);
                slow_db = pow_to_db(self.slow) + 3.0;
            }
            let knee = self.threshold_db.max(slow_db + TRANSIENT_DB);
            let target = ((fast_db - knee) * slope).clamp(0.0, MAX_GR_DB);
            let c = if target > self.gr { self.att } else { self.rel };
            self.gr += (target - self.gr) * c;
            if self.gr < 1e-3 && target == 0.0 {
                self.gr = 0.0;
            }
            *x = self.shelf.tick(*x, self.gr);
        }
    }

    fn gain_reduction_db(&self) -> f32 {
        self.gr
    }
}
