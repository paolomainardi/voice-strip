//! Stage 1: trim, polarity, 4th-order Butterworth high-pass, and a low
//! shelf compensating proximity effect.

use crate::Stage;
use crate::settings::ModuleSettings;
use crate::util::{Biquad, Coefs, Smoother, db_to_lin};

const PROXIMITY_HZ: f32 = 150.0;
// Q values of a 4th-order Butterworth split into two biquads.
const Q1: f32 = 0.541_196_1;
const Q2: f32 = 1.306_563;

pub struct Input {
    trim: Smoother,
    polarity: f32,
    hpf_hz: f32,
    hpf: [Biquad; 2],
    shelf_db: Smoother,
    shelf: Biquad,
    shelf_applied_db: f32,
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Input {
    pub fn new() -> Self {
        Self {
            trim: Smoother::new(1.0, 20.0),
            polarity: 1.0,
            hpf_hz: 0.0,
            hpf: [Biquad::default(); 2],
            shelf_db: Smoother::new(0.0, 50.0),
            shelf: Biquad::default(),
            shelf_applied_db: 0.0,
        }
    }

    pub fn set(&mut self, s: &ModuleSettings) {
        self.trim.set(db_to_lin(s.input_trim_db));
        self.polarity = if s.input_invert_polarity { -1.0 } else { 1.0 };
        if s.hpf_hz != self.hpf_hz {
            self.hpf_hz = s.hpf_hz;
            if s.hpf_hz > 0.0 {
                self.hpf[0].set(Coefs::highpass(s.hpf_hz, Q1));
                self.hpf[1].set(Coefs::highpass(s.hpf_hz, Q2));
            } else {
                for f in &mut self.hpf {
                    f.set(Coefs::IDENTITY);
                    f.reset();
                }
            }
        }
        self.shelf_db.set(s.proximity_comp_db.min(0.0));
    }
}

impl Stage for Input {
    fn latency_samples(&self) -> usize {
        0
    }

    fn reset(&mut self) {
        for f in &mut self.hpf {
            f.reset();
        }
        self.shelf.reset();
    }

    fn process(&mut self, buf: &mut [f32]) {
        // Shelf gain is updated per block from its smoother (50 ms), which
        // keeps coefficient changes small enough to be inaudible.
        for _ in 0..buf.len() {
            self.shelf_db.tick();
        }
        let shelf_db = self.shelf_db.value();
        if shelf_db != self.shelf_applied_db {
            self.shelf_applied_db = shelf_db;
            self.shelf.set(if shelf_db == 0.0 {
                Coefs::IDENTITY
            } else {
                Coefs::low_shelf(PROXIMITY_HZ, shelf_db)
            });
        }
        let hpf_on = self.hpf_hz > 0.0;
        let shelf_on = shelf_db != 0.0;
        for x in buf.iter_mut() {
            let mut y = *x * self.trim.tick() * self.polarity;
            if hpf_on {
                let h = self.hpf[0].tick(y);
                y = self.hpf[1].tick(h);
            }
            if shelf_on {
                y = self.shelf.tick(y);
            }
            *x = y;
        }
    }
}
