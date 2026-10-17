//! Stage 7: subtle tanh saturation with first-order antiderivative
//! anti-aliasing (ADAA), which suppresses aliasing without oversampling and
//! adds no lookahead. Drive 0 is an exact bypass. With drive above 0, ADAA
//! adds a half-sample group delay that is not compensated: it is below any
//! audible threshold and the stage reports 0 latency.

use crate::Stage;
use crate::settings::ModuleSettings;
use crate::util::Smoother;

pub struct Saturation {
    drive: Smoother,
    prev: f64,
}

impl Default for Saturation {
    fn default() -> Self {
        Self::new()
    }
}

/// Antiderivative of tanh: ln(cosh(x)), computed without overflow.
#[inline]
fn ln_cosh(x: f64) -> f64 {
    let a = x.abs();
    a + (-2.0 * a).exp().ln_1p() - std::f64::consts::LN_2
}

impl Saturation {
    pub fn new() -> Self {
        Self {
            drive: Smoother::new(0.0, 50.0),
            prev: 0.0,
        }
    }

    pub fn set(&mut self, s: &ModuleSettings) {
        self.drive.set(s.saturation_drive.clamp(0.0, 1.0));
    }
}

impl Stage for Saturation {
    fn latency_samples(&self) -> usize {
        0
    }

    fn reset(&mut self) {
        self.prev = 0.0;
    }

    fn process(&mut self, buf: &mut [f32]) {
        for x in buf.iter_mut() {
            let d = self.drive.tick();
            let xin = *x as f64;
            if d == 0.0 {
                self.prev = xin;
                continue;
            }
            // Drive 0..1 maps to a pre-gain of 1..4; output is normalized so
            // small signals pass at unity gain.
            let k = 1.0 + 3.0 * d as f64;
            let (a, b) = (self.prev * k, xin * k);
            let y = if (b - a).abs() < 1e-6 {
                ((a + b) * 0.5).tanh()
            } else {
                (ln_cosh(b) - ln_cosh(a)) / (b - a)
            };
            self.prev = xin;
            *x = (y / k) as f32;
        }
    }
}
