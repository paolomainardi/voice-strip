//! Stage 4: split-band de-esser. A band-pass detector at the sibilance
//! frequency drives a dynamic high shelf (corner at 0.7 x the frequency)
//! only while the band is both above threshold and dominant over the
//! full-band level. A shelf, not a subtractive band split, so the cut
//! reaches its full depth regardless of phase. Zero latency.

use crate::Stage;
use crate::settings::ModuleSettings;
use crate::util::{Biquad, Coefs, DynamicShelf, coef_ms, pow_to_db};

const RATIO: f32 = 4.0;
/// Band level must be within this many dB of the full-band level.
const DOMINANCE_DB: f32 = 6.0;
const MAX_GR_DB: f32 = 12.0;

pub struct DeEsser {
    freq: f32,
    threshold_db: f32,
    shelf: DynamicShelf,
    detect: Biquad,
    band_ms: f32,
    full_ms: f32,
    det_coef: f32,
    gr: f32,
    att: f32,
    rel: f32,
}

impl Default for DeEsser {
    fn default() -> Self {
        Self::new()
    }
}

impl DeEsser {
    pub fn new() -> Self {
        let mut s = Self {
            freq: 0.0,
            threshold_db: 0.0,
            shelf: DynamicShelf::new(4550.0, true),
            detect: Biquad::default(),
            band_ms: 0.0,
            full_ms: 0.0,
            det_coef: coef_ms(2.0),
            gr: 0.0,
            att: coef_ms(1.0),
            rel: coef_ms(60.0),
        };
        s.set_freq(6500.0);
        s
    }

    fn set_freq(&mut self, f: f32) {
        self.freq = f;
        self.shelf.set_freq(f * 0.7);
        self.detect.set(Coefs::bandpass(f, 1.5));
    }

    pub fn set(&mut self, s: &ModuleSettings) {
        if s.deesser_freq_hz != self.freq {
            self.set_freq(s.deesser_freq_hz.clamp(3000.0, 12000.0));
        }
        self.threshold_db = s.deesser_threshold_db;
    }
}

impl Stage for DeEsser {
    fn latency_samples(&self) -> usize {
        0
    }

    fn reset(&mut self) {
        self.shelf.reset();
        self.detect.reset();
        self.band_ms = 0.0;
        self.full_ms = 0.0;
        self.gr = 0.0;
    }

    fn process(&mut self, buf: &mut [f32]) {
        let slope = 1.0 - 1.0 / RATIO;
        for x in buf.iter_mut() {
            let b = self.detect.tick(*x);
            self.band_ms = crate::util::ftz(self.band_ms + (b * b - self.band_ms) * self.det_coef);
            self.full_ms =
                crate::util::ftz(self.full_ms + (*x * *x - self.full_ms) * self.det_coef);
            let band_db = pow_to_db(self.band_ms) + 3.0;
            let full_db = pow_to_db(self.full_ms) + 3.0;
            let target = if band_db > full_db - DOMINANCE_DB {
                ((band_db - self.threshold_db) * slope).clamp(0.0, MAX_GR_DB)
            } else {
                0.0
            };
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
