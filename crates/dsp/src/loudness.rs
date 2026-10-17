//! Realtime loudness metering per ITU-R BS.1770 / EBU R128: momentary
//! (400 ms), short-term (3 s), and true peak. No allocation.

use crate::util::{Biquad, Coefs, TruePeak, lin_to_db};

/// K-weighting pre-filter at 48 kHz (BS.1770-4 coefficients).
#[derive(Clone, Copy, Debug)]
pub struct KWeighting {
    shelf: Biquad,
    hpf: Biquad,
}

impl Default for KWeighting {
    fn default() -> Self {
        Self::new()
    }
}

impl KWeighting {
    pub fn new() -> Self {
        Self {
            shelf: Biquad::new(Coefs {
                b0: 1.535_124_859_586_97,
                b1: -2.691_696_189_406_38,
                b2: 1.198_392_810_852_85,
                a1: -1.690_659_293_182_41,
                a2: 0.732_480_774_215_85,
            }),
            hpf: Biquad::new(Coefs {
                b0: 1.0,
                b1: -2.0,
                b2: 1.0,
                a1: -1.990_047_454_833_98,
                a2: 0.990_072_250_366_21,
            }),
        }
    }

    pub fn reset(&mut self) {
        self.shelf.reset();
        self.hpf.reset();
    }

    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        self.hpf.tick(self.shelf.tick(x))
    }
}

const SUB: usize = 4_800;
const SHORT_SUBS: usize = 30;

pub struct LoudnessMeter {
    kw: KWeighting,
    tp: TruePeak,
    acc: f64,
    n: usize,
    subs: [f64; SHORT_SUBS],
    pos: usize,
    filled: usize,
    momentary: f32,
    short_term: f32,
    block_tp: f32,
}

impl Default for LoudnessMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl LoudnessMeter {
    pub fn new() -> Self {
        Self {
            kw: KWeighting::new(),
            tp: TruePeak::new(),
            acc: 0.0,
            n: 0,
            subs: [0.0; SHORT_SUBS],
            pos: 0,
            filled: 0,
            momentary: f32::NEG_INFINITY,
            short_term: f32::NEG_INFINITY,
            block_tp: 0.0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self {
            tp: TruePeak::new(),
            ..Self::new()
        };
    }

    /// Feeds one block. Momentary and short-term update every 100 ms;
    /// true peak is per block (call [`Self::take_true_peak_db`]).
    pub fn process(&mut self, buf: &[f32]) {
        for &x in buf {
            let k = self.kw.tick(x) as f64;
            self.acc += k * k;
            self.block_tp = self.block_tp.max(self.tp.tick(x)).max(x.abs());
            self.n += 1;
            if self.n == SUB {
                self.subs[self.pos] = self.acc / SUB as f64;
                self.pos = (self.pos + 1) % SHORT_SUBS;
                self.filled = (self.filled + 1).min(SHORT_SUBS);
                self.acc = 0.0;
                self.n = 0;
                self.momentary = self.mean_lufs(4);
                self.short_term = self.mean_lufs(SHORT_SUBS);
            }
        }
    }

    fn mean_lufs(&self, count: usize) -> f32 {
        let count = count.min(self.filled);
        if count == 0 {
            return f32::NEG_INFINITY;
        }
        let mut s = 0.0;
        for i in 0..count {
            s += self.subs[(self.pos + SHORT_SUBS - 1 - i) % SHORT_SUBS];
        }
        let p = s / count as f64;
        let lufs = if p > 0.0 {
            -0.691 + 10.0 * p.log10()
        } else {
            f64::NEG_INFINITY
        };
        // Below the BS.1770 absolute gate there is no meaningful loudness
        // (silence, a muted output, filter tails): report none.
        if lufs < -70.0 {
            f32::NEG_INFINITY
        } else {
            lufs as f32
        }
    }

    pub fn momentary(&self) -> f32 {
        self.momentary
    }

    pub fn short_term(&self) -> f32 {
        self.short_term
    }

    /// True peak since the previous call, dBTP; resets the block peak.
    pub fn take_true_peak_db(&mut self) -> f32 {
        let tp = self.block_tp;
        self.block_tp = 0.0;
        if tp > 0.0 {
            lin_to_db(tp)
        } else {
            f32::NEG_INFINITY
        }
    }
}
