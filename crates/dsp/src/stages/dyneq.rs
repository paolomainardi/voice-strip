//! Stage 5: static shaping toward the preset's target voice curve plus four
//! dynamic bands (mud, boxiness/nasality, harshness, air) that cut only
//! while the band's level relative to the full-band level exceeds its
//! threshold. Minimum-phase IIR, zero latency.

use crate::Stage;
use crate::settings::ModuleSettings;
use crate::util::{Biquad, Coefs, Smoother, coef_ms, pow_to_db};

/// Band centers (Hz) and Q. Band 3 is a high shelf for the static curve.
pub const BAND_HZ: [f32; 4] = [250.0, 1000.0, 3500.0, 10000.0];
const BAND_Q: [f32; 4] = [1.0, 1.4, 1.8, 0.7];
/// Coefficient update interval for the dynamic bands.
const UPDATE: usize = 32;
const MAX_CUT_DB: f32 = 6.0;
const DYN_RATIO: f32 = 2.0;

pub struct DynamicEq {
    static_db: [Smoother; 4],
    static_applied: [f32; 4],
    static_f: [Biquad; 4],
    thr: [f32; 4],
    detect: [Biquad; 4],
    band_ms: [f32; 4],
    full_ms: f32,
    det_coef: f32,
    cut: [f32; 4],
    cut_applied: [f32; 4],
    dyn_f: [Biquad; 4],
    att: f32,
    rel: f32,
    counter: usize,
}

impl Default for DynamicEq {
    fn default() -> Self {
        Self::new()
    }
}

fn static_coefs(band: usize, db: f32) -> Coefs {
    if db == 0.0 {
        Coefs::IDENTITY
    } else if band == 3 {
        Coefs::high_shelf(BAND_HZ[3], db)
    } else {
        Coefs::peaking(BAND_HZ[band], BAND_Q[band], db)
    }
}

impl DynamicEq {
    pub fn new() -> Self {
        Self {
            static_db: [Smoother::new(0.0, 80.0); 4],
            static_applied: [0.0; 4],
            static_f: [Biquad::default(); 4],
            thr: [f32::INFINITY; 4],
            detect: std::array::from_fn(|i| Biquad::new(Coefs::bandpass(BAND_HZ[i], BAND_Q[i]))),
            band_ms: [0.0; 4],
            full_ms: 0.0,
            det_coef: coef_ms(20.0),
            cut: [0.0; 4],
            cut_applied: [0.0; 4],
            dyn_f: [Biquad::default(); 4],
            att: coef_ms(15.0) * UPDATE as f32,
            rel: coef_ms(150.0) * UPDATE as f32,
            counter: 0,
        }
    }

    pub fn set(&mut self, s: &ModuleSettings) {
        for i in 0..4 {
            self.static_db[i].set(s.eq_static_db[i]);
            // A threshold of 0 dB relative is unreachable for a band-pass
            // inside the full band, so it doubles as "off".
            self.thr[i] = if s.eq_dynamic_threshold_db[i] >= 0.0 {
                f32::INFINITY
            } else {
                s.eq_dynamic_threshold_db[i]
            };
        }
    }

    fn update(&mut self) {
        let full_db = pow_to_db(self.full_ms);
        for i in 0..4 {
            for _ in 0..UPDATE {
                self.static_db[i].tick();
            }
            let sdb = self.static_db[i].value();
            if sdb != self.static_applied[i] {
                self.static_applied[i] = sdb;
                self.static_f[i].set(static_coefs(i, sdb));
            }
            let rel_db = pow_to_db(self.band_ms[i]) - full_db;
            let target = if self.full_ms > 1e-10 {
                ((rel_db - self.thr[i]) * (1.0 - 1.0 / DYN_RATIO)).clamp(0.0, MAX_CUT_DB)
            } else {
                0.0
            };
            let c = if target > self.cut[i] {
                self.att
            } else {
                self.rel
            };
            self.cut[i] += (target - self.cut[i]) * c.min(1.0);
            if self.cut[i] < 0.01 && target == 0.0 {
                self.cut[i] = 0.0;
            }
            if (self.cut[i] - self.cut_applied[i]).abs() > 0.01
                || (self.cut[i] == 0.0 && self.cut_applied[i] != 0.0)
            {
                self.cut_applied[i] = self.cut[i];
                self.dyn_f[i].set(if self.cut[i] == 0.0 {
                    Coefs::IDENTITY
                } else {
                    Coefs::peaking(BAND_HZ[i], BAND_Q[i], -self.cut[i])
                });
            }
        }
    }

    pub fn cuts_db(&self) -> [f32; 4] {
        self.cut
    }
}

impl Stage for DynamicEq {
    fn latency_samples(&self) -> usize {
        0
    }

    fn reset(&mut self) {
        for i in 0..4 {
            self.static_f[i].reset();
            self.detect[i].reset();
            self.dyn_f[i].reset();
        }
        self.band_ms = [0.0; 4];
        self.full_ms = 0.0;
        self.cut = [0.0; 4];
    }

    fn process(&mut self, buf: &mut [f32]) {
        let dyn_on = self.thr.iter().any(|t| t.is_finite());
        for x in buf.iter_mut() {
            if self.counter == 0 {
                self.update();
                self.counter = UPDATE;
            }
            self.counter -= 1;
            let mut y = *x;
            if dyn_on {
                self.full_ms =
                    crate::util::ftz(self.full_ms + (y * y - self.full_ms) * self.det_coef);
                for i in 0..4 {
                    if self.thr[i].is_finite() {
                        let b = self.detect[i].tick(y);
                        self.band_ms[i] = crate::util::ftz(
                            self.band_ms[i] + (b * b - self.band_ms[i]) * self.det_coef,
                        );
                    }
                }
            }
            for i in 0..4 {
                if self.static_applied[i] != 0.0 {
                    y = self.static_f[i].tick(y);
                }
                if self.cut_applied[i] != 0.0 {
                    y = self.dyn_f[i].tick(y);
                }
            }
            *x = y;
        }
    }

    fn gain_reduction_db(&self) -> f32 {
        self.cut.iter().fold(0.0f32, |a, &b| a.max(b))
    }
}
