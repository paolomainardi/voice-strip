//! Shared DSP primitives: conversions, parameter smoothing, biquads.

use std::f64::consts::PI;

use crate::SAMPLE_RATE;

const FS: f64 = SAMPLE_RATE as f64;

#[inline]
pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db * 0.05)
}

#[inline]
pub fn lin_to_db(x: f32) -> f32 {
    20.0 * x.max(1e-12).log10()
}

#[inline]
pub fn pow_to_db(p: f32) -> f32 {
    10.0 * p.max(1e-24).log10()
}

/// One-pole coefficient for a time constant in milliseconds.
#[inline]
pub fn coef_ms(ms: f32) -> f32 {
    if ms <= 0.0 {
        1.0
    } else {
        1.0 - (-1.0 / (ms * 0.001 * SAMPLE_RATE as f32)).exp()
    }
}

/// Flushes values in the denormal range to zero (envelope followers decay
/// into denormals on digital silence, which is slow on x86).
#[inline]
pub fn ftz(x: f32) -> f32 {
    if x.abs() < 1e-30 { 0.0 } else { x }
}

/// Flushes denormals in filter state to zero.
#[inline]
fn flush(x: f64) -> f64 {
    if x.abs() < 1e-30 { 0.0 } else { x }
}

/// One-pole parameter smoother. `next` returns the exact target once the
/// distance falls below a small epsilon, so neutral settings stay bit-exact.
#[derive(Clone, Copy, Debug)]
pub struct Smoother {
    value: f32,
    target: f32,
    coef: f32,
}

impl Smoother {
    pub fn new(value: f32, ms: f32) -> Self {
        Self {
            value,
            target: value,
            coef: coef_ms(ms),
        }
    }

    pub fn set(&mut self, target: f32) {
        self.target = target;
    }

    pub fn snap(&mut self, value: f32) {
        self.value = value;
        self.target = value;
    }

    #[inline]
    pub fn tick(&mut self) -> f32 {
        if self.value != self.target {
            let v = self.value + (self.target - self.value) * self.coef;
            // Snap when close or when f32 precision stalls the approach.
            self.value = if (self.target - v).abs() < 1e-5 || v == self.value {
                self.target
            } else {
                v
            };
        }
        self.value
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn is_settled(&self) -> bool {
        self.value == self.target
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coefs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coefs {
    pub const IDENTITY: Coefs = Coefs {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn norm(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    fn w(freq: f32) -> (f64, f64) {
        let w0 = 2.0 * PI * (freq as f64).clamp(1.0, FS * 0.49) / FS;
        (w0.cos(), w0.sin())
    }

    pub fn highpass(freq: f32, q: f32) -> Self {
        let (c, s) = Self::w(freq);
        let al = s / (2.0 * q as f64);
        Self::norm(
            (1.0 + c) / 2.0,
            -(1.0 + c),
            (1.0 + c) / 2.0,
            1.0 + al,
            -2.0 * c,
            1.0 - al,
        )
    }

    pub fn lowpass(freq: f32, q: f32) -> Self {
        let (c, s) = Self::w(freq);
        let al = s / (2.0 * q as f64);
        Self::norm(
            (1.0 - c) / 2.0,
            1.0 - c,
            (1.0 - c) / 2.0,
            1.0 + al,
            -2.0 * c,
            1.0 - al,
        )
    }

    /// Band-pass with 0 dB peak gain.
    pub fn bandpass(freq: f32, q: f32) -> Self {
        let (c, s) = Self::w(freq);
        let al = s / (2.0 * q as f64);
        Self::norm(al, 0.0, -al, 1.0 + al, -2.0 * c, 1.0 - al)
    }

    pub fn peaking(freq: f32, q: f32, gain_db: f32) -> Self {
        let (c, s) = Self::w(freq);
        let a = 10f64.powf(gain_db as f64 / 40.0);
        let al = s / (2.0 * q as f64);
        Self::norm(
            1.0 + al * a,
            -2.0 * c,
            1.0 - al * a,
            1.0 + al / a,
            -2.0 * c,
            1.0 - al / a,
        )
    }

    /// Low shelf, slope 1.
    pub fn low_shelf(freq: f32, gain_db: f32) -> Self {
        let (c, s) = Self::w(freq);
        let a = 10f64.powf(gain_db as f64 / 40.0);
        let al = s / 2.0 * 2f64.sqrt();
        let sa = 2.0 * a.sqrt() * al;
        Self::norm(
            a * ((a + 1.0) - (a - 1.0) * c + sa),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - sa),
            (a + 1.0) + (a - 1.0) * c + sa,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - sa,
        )
    }

    /// High shelf, slope 1.
    pub fn high_shelf(freq: f32, gain_db: f32) -> Self {
        let (c, s) = Self::w(freq);
        let a = 10f64.powf(gain_db as f64 / 40.0);
        let al = s / 2.0 * 2f64.sqrt();
        let sa = 2.0 * a.sqrt() * al;
        Self::norm(
            a * ((a + 1.0) + (a - 1.0) * c + sa),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - sa),
            (a + 1.0) - (a - 1.0) * c + sa,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - sa,
        )
    }

    /// Magnitude response in dB at `freq`.
    pub fn magnitude_db(&self, freq: f32) -> f32 {
        let w = 2.0 * PI * freq as f64 / FS;
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        (10.0 * ((nr * nr + ni * ni) / (dr * dr + di * di)).log10()) as f32
    }
}

/// Transposed direct form II biquad with f64 state.
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    c: Coefs,
    z1: f64,
    z2: f64,
}

impl Default for Biquad {
    fn default() -> Self {
        Self::new(Coefs::IDENTITY)
    }
}

impl Biquad {
    pub fn new(c: Coefs) -> Self {
        Self {
            c,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Changes coefficients keeping state (safe for small, smoothed changes).
    pub fn set(&mut self, c: Coefs) {
        self.c = c;
    }

    pub fn coefs(&self) -> &Coefs {
        &self.c
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let x = x as f64;
        let c = &self.c;
        let y = c.b0 * x + self.z1;
        self.z1 = flush(c.b1 * x - c.a1 * y + self.z2);
        self.z2 = flush(c.b2 * x - c.a2 * y);
        y as f32
    }
}

/// Shelf whose cut depth follows a gain-reduction value, with coefficients
/// refreshed every `UPDATE` samples while the depth moves. Zero depth is an
/// exact bypass.
#[derive(Clone, Copy, Debug)]
pub struct DynamicShelf {
    freq: f32,
    high: bool,
    filter: Biquad,
    applied_db: f32,
    countdown: usize,
}

impl DynamicShelf {
    const UPDATE: usize = 4;

    pub fn new(freq: f32, high: bool) -> Self {
        Self {
            freq,
            high,
            filter: Biquad::default(),
            applied_db: 0.0,
            countdown: 0,
        }
    }

    pub fn set_freq(&mut self, freq: f32) {
        self.freq = freq;
        self.applied_db = f32::NAN;
    }

    pub fn reset(&mut self) {
        self.filter.reset();
        self.filter.set(Coefs::IDENTITY);
        self.applied_db = 0.0;
        self.countdown = 0;
    }

    /// Filters one sample with a cut of `cut_db` (>= 0).
    #[inline]
    pub fn tick(&mut self, x: f32, cut_db: f32) -> f32 {
        if self.countdown == 0 {
            self.countdown = Self::UPDATE;
            let moved = (cut_db - self.applied_db).abs() > 0.05
                || (cut_db == 0.0 && self.applied_db != 0.0)
                || self.applied_db.is_nan();
            if moved {
                self.applied_db = cut_db;
                self.filter.set(if cut_db == 0.0 {
                    Coefs::IDENTITY
                } else if self.high {
                    Coefs::high_shelf(self.freq, -cut_db)
                } else {
                    Coefs::low_shelf(self.freq, -cut_db)
                });
            }
        }
        self.countdown -= 1;
        if self.applied_db == 0.0 {
            // Keep the state flowing so re-engaging is seamless.
            self.filter.tick(x);
            x
        } else {
            self.filter.tick(x)
        }
    }
}

/// Fixed-capacity delay line. Allocates only in `new`.
#[derive(Clone, Debug)]
pub struct Delay {
    buf: Vec<f32>,
    pos: usize,
}

impl Delay {
    /// A delay of `len` samples; 0 is a passthrough.
    pub fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len],
            pos: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }

    /// Pushes `x` and returns the sample delayed by `len` samples.
    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        if self.buf.is_empty() {
            return x;
        }
        let y = self.buf[self.pos];
        self.buf[self.pos] = x;
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
        y
    }
}

const TP_TAPS: usize = 24;

/// 4x polyphase interpolator for true-peak detection: 96 taps (24 per
/// phase), Kaiser-windowed sinc, flat to about 20 kHz so it does not
/// under-read high-frequency content like the 48-tap BS.1770 example.
#[derive(Clone, Debug)]
pub struct TruePeak {
    h: [[f32; TP_TAPS]; 4],
    hist: [f32; TP_TAPS],
    pos: usize,
}

impl Default for TruePeak {
    fn default() -> Self {
        Self::new()
    }
}

impl TruePeak {
    pub fn new() -> Self {
        // Windowed-sinc low-pass at the original Nyquist, Kaiser beta 8.
        const N: usize = 4 * TP_TAPS;
        let mut h = [[0.0f32; TP_TAPS]; 4];
        let center = (N as f64 - 1.0) / 2.0;
        let beta = 8.0;
        let i0b = bessel_i0(beta);
        for (n, _) in (0..N).enumerate() {
            let t = (n as f64 - center) / 4.0;
            let sinc = if t.abs() < 1e-12 {
                1.0
            } else {
                (PI * t).sin() / (PI * t)
            };
            let r = (n as f64 - center) / center;
            let w = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / i0b;
            h[n % 4][n / 4] = (sinc * w) as f32;
        }
        // Normalize each phase to unity DC gain.
        for phase in h.iter_mut() {
            let s: f32 = phase.iter().sum();
            for c in phase.iter_mut() {
                *c /= s;
            }
        }
        Self {
            h,
            hist: [0.0; TP_TAPS],
            pos: 0,
        }
    }

    pub fn reset(&mut self) {
        self.hist = [0.0; TP_TAPS];
        self.pos = 0;
    }

    /// Detector support: the value at time n depends on inputs n-SUPPORT+1..=n.
    pub const SUPPORT: usize = TP_TAPS;

    /// Pushes one input sample, returns the absolute peak of the 4
    /// interpolated values around sample `n - DELAY`.
    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        self.pos = if self.pos == 0 {
            TP_TAPS - 1
        } else {
            self.pos - 1
        };
        self.hist[self.pos] = x;
        let mut peak = 0.0f32;
        for phase in &self.h {
            let mut acc = 0.0f32;
            for (k, c) in phase.iter().enumerate() {
                acc += c * self.hist[(self.pos + k) % TP_TAPS];
            }
            peak = peak.max(acc.abs());
        }
        peak
    }
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let mut k = 1.0;
    while term > 1e-12 * sum {
        term *= (x / (2.0 * k)) * (x / (2.0 * k));
        sum += term;
        k += 1.0;
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highpass_response() {
        let c = Coefs::highpass(100.0, std::f32::consts::FRAC_1_SQRT_2);
        assert!((c.magnitude_db(100.0) + 3.01).abs() < 0.05);
        assert!(c.magnitude_db(1000.0).abs() < 0.01);
        assert!(c.magnitude_db(25.0) < -23.0);
    }

    #[test]
    fn peaking_gain_at_center() {
        let c = Coefs::peaking(3000.0, 1.0, 4.0);
        assert!((c.magnitude_db(3000.0) - 4.0).abs() < 0.01);
        assert!(c.magnitude_db(100.0).abs() < 0.1);
    }

    #[test]
    fn shelves() {
        let lo = Coefs::low_shelf(150.0, -6.0);
        assert!((lo.magnitude_db(20.0) + 6.0).abs() < 0.2);
        assert!(lo.magnitude_db(5000.0).abs() < 0.05);
        let hi = Coefs::high_shelf(8000.0, 3.0);
        assert!((hi.magnitude_db(20000.0) - 3.0).abs() < 0.3);
        assert!(hi.magnitude_db(200.0).abs() < 0.05);
    }

    #[test]
    fn smoother_reaches_target_exactly() {
        let mut s = Smoother::new(0.0, 5.0);
        s.set(1.0);
        for _ in 0..48_000 {
            s.tick();
        }
        assert_eq!(s.value(), 1.0);
    }

    #[test]
    fn true_peak_finds_intersample_peak() {
        // fs/4 sine at 45 degrees phase: samples peak at 0.707, true peak 1.0.
        let mut tp = TruePeak::new();
        let mut m = 0.0f32;
        for n in 0..1000 {
            let x = (std::f32::consts::FRAC_PI_2 * n as f32 + std::f32::consts::FRAC_PI_4).sin();
            m = m.max(tp.tick(x));
        }
        assert!((lin_to_db(m)).abs() < 0.1, "true peak {m}");
    }
}
