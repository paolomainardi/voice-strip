//! Spectrum analyzer data, in the manner of the analyzers in Logic's Channel
//! EQ or FabFilter Pro-Q:
//!
//! - **Two resolutions.** An 8192-point FFT (5.9 Hz bins) below 300 Hz and a
//!   2048-point one (23 Hz bins, faster) above 424 Hz, cross-faded over the
//!   half octave between them. Both run at a fixed hop, so the analysis
//!   rate does not depend on the frame rate.
//! - **Display points.** [`POINTS`] log-spaced points from 20 Hz to 20 kHz.
//!   Where bins are sparser than points, the level is interpolated with a
//!   monotone cubic (PCHIP) on the log-frequency axis, so a low tone is one
//!   smooth peak with no steps and no overshoot. Where a point covers bins,
//!   it takes their maximum.
//! - **Smoothing and tilt.** Fractional-octave smoothing in the power
//!   domain fills the valleys between partials without lowering peaks, and
//!   a +4.5 dB/octave tilt around 1 kHz makes speech and music read
//!   roughly flat, as in Pro-Q and SPAN.
//! - **Ballistics.** Per point, in dB, time based: fast attack, linear
//!   release, and a peak hold that falls after a while.
//!
//! The FFT runs on the UI thread, never on the audio thread. No allocation
//! after construction.

use std::f32::consts::PI;
use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex};

pub const F_MIN: f32 = 20.0;
pub const F_MAX: f32 = 20_000.0;
/// Display points from [`F_MIN`] to [`F_MAX`].
pub const POINTS: usize = 400;
/// Bands of the EQ action curve.
pub const BANDS: usize = 96;

pub const FFT_LOW: usize = 8192;
pub const FFT_HIGH: usize = 2048;
/// Hops: about 47 low and 94 high spectra per second at 48 kHz.
const HOP_LOW: usize = 1024;
const HOP_HIGH: usize = 512;
/// The low FFT alone below this, the high one alone above `CROSS_HI`.
const CROSS_LO: f32 = 300.0;
const CROSS_HI: f32 = 424.0;
/// Most analyses per resolution in one update; older hops are skipped.
const MAX_HOPS: usize = 4;

/// Fractional-octave smoothing width.
pub const SMOOTHING_OCTAVES: f32 = 1.0 / 6.0;
/// Display tilt around [`TILT_PIVOT_HZ`].
pub const TILT_DB_PER_OCTAVE: f32 = 4.5;
pub const TILT_PIVOT_HZ: f32 = 1000.0;

/// Attack time constant, seconds.
const ATTACK_S: f32 = 0.010;
/// Release rate, dB per second.
pub const RELEASE_DB_PER_S: f32 = 36.0;
/// Peak hold time, seconds, and how fast the held peak then falls.
const HOLD_S: f32 = 1.5;
const HOLD_FALL_DB_PER_S: f32 = 20.0;

pub const SILENCE_DB: f32 = -120.0;

/// Hann window: ENBW 1.5 bins; coherent gain 0.5.
const HANN_GAIN: f32 = 0.5;

/// One resolution: an FFT over the most recent `n` samples.
struct Fft {
    n: usize,
    hop: usize,
    plan: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// Level of each bin, dB; a full-scale sine reads 0 dB.
    bins_db: Vec<f32>,
    /// Samples pushed since the last analysis.
    pending: usize,
    /// Analyses run so far (tests).
    count: u64,
}

impl Fft {
    fn new(planner: &mut RealFftPlanner<f32>, n: usize, hop: usize) -> Self {
        let plan = planner.plan_fft_forward(n);
        let input = plan.make_input_vec();
        let output = plan.make_output_vec();
        let scratch = plan.make_scratch_vec();
        let window = (0..n)
            .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / n as f32).cos())
            .collect();
        Self {
            n,
            hop,
            plan,
            window,
            input,
            output,
            scratch,
            bins_db: vec![SILENCE_DB; n / 2 + 1],
            pending: 0,
            count: 0,
        }
    }

    fn bin_hz(&self, sample_rate: f32) -> f32 {
        sample_rate / self.n as f32
    }

    /// Runs one analysis per hop that has come since the last update (at
    /// most [`MAX_HOPS`]), keeping the loudest reading of each bin so a
    /// short transient between frames still shows. Returns whether any ran.
    fn analyze_pending(&mut self, ring: &[f32], ring_pos: usize) -> bool {
        if self.pending > MAX_HOPS * self.hop {
            self.pending = self.hop * (MAX_HOPS - 1) + self.pending % self.hop;
        }
        let mut ran = false;
        while self.pending >= self.hop {
            self.pending -= self.hop;
            // This hop ends `pending` samples before the newest one.
            let end = (ring_pos + ring.len() - self.pending) % ring.len();
            self.analyze(ring, end, ran);
            ran = true;
        }
        ran
    }

    /// Analyzes the `n` samples of `ring` that end just before `end`; with
    /// `merge`, keeps the louder of the new and the previous bin levels.
    fn analyze(&mut self, ring: &[f32], end: usize, merge: bool) {
        let len = ring.len();
        let start = (end + len - self.n) % len;
        for i in 0..self.n {
            self.input[i] = ring[(start + i) % len] * self.window[i];
        }
        if self
            .plan
            .process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch)
            .is_err()
        {
            return;
        }
        let norm = 2.0 / (HANN_GAIN * self.n as f32);
        for (db, c) in self.bins_db.iter_mut().zip(&self.output) {
            let mag = c.norm() * norm;
            let v = if mag > 0.0 {
                (20.0 * mag.log10()).max(SILENCE_DB)
            } else {
                SILENCE_DB
            };
            *db = if merge { db.max(v) } else { v };
        }
        self.count += 1;
    }

    /// Level at frequency `f` for a display point covering `[lo, hi]` Hz.
    fn level_db(&self, f: f32, lo: f32, hi: f32, sample_rate: f32) -> f32 {
        let bin_hz = self.bin_hz(sample_rate);
        let last = self.bins_db.len() - 2;
        let (k_lo, k_hi) = (
            (lo / bin_hz).ceil() as usize,
            (hi / bin_hz).floor() as usize,
        );
        if k_hi >= k_lo && k_lo >= 1 {
            // The point covers bins: keep the loudest.
            return self.bins_db[k_lo.min(last)..=k_hi.min(last)]
                .iter()
                .copied()
                .fold(SILENCE_DB, f32::max);
        }
        // Bins are sparser than points: monotone cubic between bin centres
        // on the log-frequency axis. DC is left out.
        let pos = f / bin_hz;
        let k = (pos.floor() as usize).clamp(1, last - 1);
        let x = |i: usize| (i as f32 * bin_hz).ln();
        let y = |i: usize| self.bins_db[i];
        let i0 = k.saturating_sub(1).max(1);
        let i3 = (k + 2).min(last + 1);
        pchip(
            [x(i0), x(k), x(k + 1), x(i3)],
            [y(i0), y(k), y(k + 1), y(i3)],
            f.ln(),
        )
    }
}

/// Monotone cubic Hermite (Fritsch-Carlson) on the middle interval of four
/// points; `xs` increasing (the outer ones may repeat the inner ones at the
/// ends of the range).
fn pchip(xs: [f32; 4], ys: [f32; 4], x: f32) -> f32 {
    let h1 = xs[2] - xs[1];
    if h1 <= 0.0 {
        return ys[1];
    }
    let d1 = (ys[2] - ys[1]) / h1;
    let slope = |a: usize, b: usize| {
        let h = xs[b] - xs[a];
        if h > 0.0 { (ys[b] - ys[a]) / h } else { d1 }
    };
    let (d0, d2) = (slope(0, 1), slope(2, 3));
    let tangent = |da: f32, db: f32, ha: f32, hb: f32| {
        if da * db <= 0.0 {
            0.0
        } else {
            // Weighted harmonic mean keeps the curve inside the data.
            let (w1, w2) = (2.0 * hb + ha, hb + 2.0 * ha);
            (w1 + w2) / (w1 / da + w2 / db)
        }
    };
    let h0 = (xs[1] - xs[0]).max(1e-9);
    let h2 = (xs[3] - xs[2]).max(1e-9);
    let m1 = tangent(d0, d1, h0, h1);
    let m2 = tangent(d1, d2, h1, h2);
    let t = ((x - xs[1]) / h1).clamp(0.0, 1.0);
    let (t2, t3) = (t * t, t * t * t);
    (2.0 * t3 - 3.0 * t2 + 1.0) * ys[1]
        + (t3 - 2.0 * t2 + t) * h1 * m1
        + (-2.0 * t3 + 3.0 * t2) * ys[2]
        + (t3 - t2) * h1 * m2
}

/// Frequency of display point `i`, Hz.
pub fn point_hz(i: usize) -> f32 {
    F_MIN * (F_MAX / F_MIN).powf(i as f32 / (POINTS - 1) as f32)
}

/// Display tilt at `f`, dB.
pub fn tilt_db(f: f32) -> f32 {
    TILT_DB_PER_OCTAVE * (f / TILT_PIVOT_HZ).log2()
}

/// Weight of the low-resolution FFT at `f`: 1 below the cross-fade, 0
/// above, a raised cosine on the log axis in between.
fn low_weight(f: f32) -> f32 {
    if f <= CROSS_LO {
        1.0
    } else if f >= CROSS_HI {
        0.0
    } else {
        let t = (f / CROSS_LO).ln() / (CROSS_HI / CROSS_LO).ln();
        0.5 + 0.5 * (PI * t).cos()
    }
}

pub struct Analyzer {
    sample_rate: f32,
    ring: Vec<f32>,
    ring_pos: usize,
    low: Fft,
    high: Fft,
    /// Unsmoothed level per point from the latest spectra, dB (scratch).
    raw: Vec<f32>,
    /// Target per point after smoothing and tilt, dB.
    target: Vec<f32>,
    /// Displayed level per point after ballistics, dB.
    points: Vec<f32>,
    /// Held peaks per point, dB, and how long each has been held, s.
    peaks: Vec<f32>,
    held: Vec<f32>,
    /// Broadband level of the latest spectrum, dB.
    level_db: f32,
    /// Slow minimum of the broadband level: the noise floor estimate.
    noise_db: f32,
}

impl Analyzer {
    pub fn new(sample_rate: f32) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        Self {
            sample_rate,
            // Room for the longest FFT ending `MAX_HOPS` hops back.
            ring: vec![0.0; FFT_LOW + MAX_HOPS * HOP_LOW],
            ring_pos: 0,
            low: Fft::new(&mut planner, FFT_LOW, HOP_LOW),
            high: Fft::new(&mut planner, FFT_HIGH, HOP_HIGH),
            raw: vec![SILENCE_DB; POINTS],
            target: vec![SILENCE_DB; POINTS],
            points: vec![SILENCE_DB; POINTS],
            peaks: vec![SILENCE_DB; POINTS],
            held: vec![0.0; POINTS],
            level_db: SILENCE_DB,
            noise_db: SILENCE_DB,
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn push(&mut self, samples: &[f32]) {
        let len = self.ring.len();
        for &s in samples {
            self.ring[self.ring_pos] = s;
            self.ring_pos = (self.ring_pos + 1) % len;
        }
        self.low.pending += samples.len();
        self.high.pending += samples.len();
    }

    /// Displayed levels in dB at [`point_hz`], low to high frequency.
    pub fn points(&self) -> &[f32] {
        &self.points
    }

    /// Held peaks in dB at [`point_hz`].
    pub fn peaks(&self) -> &[f32] {
        &self.peaks
    }

    pub fn noise_floor_db(&self) -> f32 {
        self.noise_db
    }

    pub fn level_db(&self) -> f32 {
        self.level_db
    }

    /// Spectra analyzed so far at each resolution (low, high).
    pub fn analyses(&self) -> (u64, u64) {
        (self.low.count, self.high.count)
    }

    /// Overrides the display with fixed values (screenshots, tests); `f`
    /// gets the frequency and returns the displayed dB.
    pub fn set_demo(&mut self, f: impl Fn(f32) -> f32, noise_db: f32) {
        for i in 0..POINTS {
            let v = f(point_hz(i));
            self.target[i] = v;
            self.points[i] = v;
            self.peaks[i] = v;
        }
        self.noise_db = noise_db;
        self.level_db = noise_db + 40.0;
    }

    /// Analyzes the spectra whose hop has come and advances the display
    /// ballistics by `dt` seconds.
    pub fn update(&mut self, dt: f32) {
        let low = self.low.analyze_pending(&self.ring, self.ring_pos);
        let high = self.high.analyze_pending(&self.ring, self.ring_pos);
        if high {
            self.level_db = self.broadband_db();
        }
        let fresh = low || high;
        if fresh {
            self.compute_targets();
        }
        self.ballistics(dt.max(0.0));
        if self.level_db < self.noise_db || self.noise_db <= SILENCE_DB {
            self.noise_db = self.level_db;
        } else {
            // Let the estimate drift up slowly so it follows a changing room.
            self.noise_db = (self.noise_db + 2.0 * dt).min(self.level_db);
        }
    }

    /// Power of the high-resolution spectrum from 20 Hz to 20 kHz, dB.
    fn broadband_db(&self) -> f32 {
        let bin_hz = self.high.bin_hz(self.sample_rate);
        let lo = ((F_MIN / bin_hz).ceil() as usize).max(1);
        let hi = ((F_MAX / bin_hz).floor() as usize).min(self.high.bins_db.len() - 1);
        let total: f32 = self.high.bins_db[lo..=hi]
            .iter()
            .map(|db| 10f32.powf(db / 10.0))
            .sum();
        if total > 0.0 {
            (10.0 * total.log10()).max(SILENCE_DB)
        } else {
            SILENCE_DB
        }
    }

    fn compute_targets(&mut self) {
        let ratio = (F_MAX / F_MIN).powf(1.0 / (POINTS - 1) as f32);
        let half = ratio.sqrt();
        for i in 0..POINTS {
            let f = point_hz(i);
            let (lo, hi) = (f / half, f * half);
            let w = low_weight(f);
            let mut power = 0.0;
            if w > 0.0 {
                let db = self.low.level_db(f, lo, hi, self.sample_rate);
                power += w * 10f32.powf(db / 10.0);
            }
            if w < 1.0 {
                let db = self.high.level_db(f, lo, hi, self.sample_rate);
                power += (1.0 - w) * 10f32.powf(db / 10.0);
            }
            self.raw[i] = if power > 0.0 {
                (10.0 * power.log10()).max(SILENCE_DB)
            } else {
                SILENCE_DB
            };
        }
        // Fractional-octave smoothing in power, as a floor under the raw
        // curve: valleys between partials fill in, peaks keep their level.
        let span = ((SMOOTHING_OCTAVES / 2.0) / ratio.log2()).round() as usize;
        for i in 0..POINTS {
            let (a, b) = (i.saturating_sub(span), (i + span).min(POINTS - 1));
            let mean = self.raw[a..=b]
                .iter()
                .map(|db| 10f32.powf(db / 10.0))
                .sum::<f32>()
                / (b - a + 1) as f32;
            let smooth = if mean > 0.0 {
                10.0 * mean.log10()
            } else {
                SILENCE_DB
            };
            let level = self.raw[i].max(smooth);
            self.target[i] = if level <= SILENCE_DB {
                SILENCE_DB
            } else {
                level + tilt_db(point_hz(i))
            };
        }
    }

    fn ballistics(&mut self, dt: f32) {
        let attack = 1.0 - (-dt / ATTACK_S).exp();
        for i in 0..POINTS {
            let (cur, target) = (self.points[i], self.target[i]);
            let v = if target > cur {
                cur + (target - cur) * attack
            } else {
                (cur - RELEASE_DB_PER_S * dt).max(target)
            };
            self.points[i] = v;
            if v >= self.peaks[i] {
                self.peaks[i] = v;
                self.held[i] = 0.0;
            } else {
                self.held[i] += dt;
                if self.held[i] > HOLD_S {
                    self.peaks[i] = (self.peaks[i] - HOLD_FALL_DB_PER_S * dt).max(v);
                }
            }
        }
    }
}

/// Centre of EQ action band `b`, Hz.
pub fn band_center_hz(b: usize) -> f32 {
    F_MIN * (F_MAX / F_MIN).powf((b as f32 + 0.5) / BANDS as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sine(f: f32, amp: f32, n: usize, offset: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * PI * f * (i + offset) as f32 / SR).sin())
            .collect()
    }

    /// Feeds `seconds` of `signal(offset, n)` at `fps`.
    fn run(a: &mut Analyzer, seconds: f32, fps: f32, signal: impl Fn(usize, usize) -> Vec<f32>) {
        let per_frame = (SR / fps) as usize;
        let frames = (seconds * fps) as usize;
        for k in 0..frames {
            a.push(&signal(k * per_frame, per_frame));
            a.update(1.0 / fps);
        }
    }

    /// Display point nearest to `f`.
    fn nearest(f: f32) -> usize {
        (0..POINTS)
            .min_by(|&a, &b| {
                (point_hz(a) / f)
                    .ln()
                    .abs()
                    .total_cmp(&(point_hz(b) / f).ln().abs())
            })
            .unwrap()
    }

    #[test]
    fn sine_reads_its_level_at_its_frequency() {
        for f in [60.0f32, 150.0, 1000.0, 5000.0] {
            let mut a = Analyzer::new(SR);
            run(&mut a, 1.0, 60.0, |o, n| sine(f, 0.5, n, o));
            let (best, &db) = a
                .points()
                .iter()
                .enumerate()
                .max_by(|x, y| x.1.total_cmp(y.1))
                .unwrap();
            let fp = point_hz(best);
            assert!((fp / f).log2().abs() < 0.05, "{f} Hz peaks at {fp} Hz");
            // -6 dB sine, plus the display tilt.
            let want = -6.02 + tilt_db(fp);
            assert!((db - want).abs() < 1.5, "{f} Hz reads {db} dB, want {want}");
        }
    }

    #[test]
    fn low_sine_is_smooth_not_stepped() {
        let mut a = Analyzer::new(SR);
        run(&mut a, 1.0, 60.0, |o, n| sine(60.0, 0.5, n, o));
        // The curve before tilt, which would otherwise hide flat steps.
        let raw = &a.raw;
        let (lo, hi) = (nearest(25.0), nearest(200.0));
        let flat = (lo..hi)
            .filter(|&i| (raw[i + 1] - raw[i]).abs() < 0.01)
            .count();
        assert_eq!(flat, 0, "{flat} flat steps below 200 Hz");
        // No interpolation overshoot above the analyzed bins.
        let peak = raw[lo..hi].iter().copied().fold(f32::MIN, f32::max);
        let bin_peak = a.low.bins_db.iter().copied().fold(f32::MIN, f32::max);
        assert!(
            peak <= bin_peak + 0.1,
            "{peak} dB over bins at {bin_peak} dB"
        );
    }

    #[test]
    fn analysis_rate_is_fixed() {
        let mut a = Analyzer::new(SR);
        run(&mut a, 2.0, 60.0, |o, n| sine(1000.0, 0.5, n, o));
        let mut b = Analyzer::new(SR);
        run(&mut b, 2.0, 120.0, |o, n| sine(1000.0, 0.5, n, o));
        let (la, ha) = a.analyses();
        let (lb, hb) = b.analyses();
        assert!(
            la.abs_diff(lb) <= 2 && ha.abs_diff(hb) <= 2,
            "{la}/{ha} vs {lb}/{hb}"
        );
        assert!((90..=95).contains(&(ha / 2)), "{ha} high spectra in 2 s");
    }

    #[test]
    fn ballistics_are_frame_rate_independent() {
        let mut a = Analyzer::new(SR);
        run(&mut a, 1.0, 60.0, |o, n| sine(1000.0, 0.5, n, o));
        let mut b = Analyzer::new(SR);
        run(&mut b, 1.0, 120.0, |o, n| sine(1000.0, 0.5, n, o));
        let i = nearest(1000.0);
        assert!((a.points()[i] - b.points()[i]).abs() < 0.5);
        // Release: a 60 dB drop takes 60 / rate seconds.
        let level = a.points()[i];
        run(&mut a, 1.0, 60.0, |_, n| vec![0.0; n]);
        let fallen = level - a.points()[i];
        assert!(fallen >= RELEASE_DB_PER_S * 0.95, "fell {fallen} dB in 1 s");
    }

    #[test]
    fn peaks_hold_then_fall() {
        let mut a = Analyzer::new(SR);
        run(&mut a, 0.5, 60.0, |o, n| sine(1000.0, 0.5, n, o));
        let i = nearest(1000.0);
        let peak = a.peaks()[i];
        run(&mut a, 1.0, 60.0, |_, n| vec![0.0; n]);
        assert!((a.peaks()[i] - peak).abs() < 0.01, "held peak moved");
        run(&mut a, 2.0, 60.0, |_, n| vec![0.0; n]);
        assert!(a.peaks()[i] < peak - 10.0, "held peak never fell");
    }

    #[test]
    fn tilt_pivots_at_1_khz() {
        assert_eq!(tilt_db(1000.0), 0.0);
        assert!((tilt_db(2000.0) - 4.5).abs() < 1e-4);
        assert!((tilt_db(500.0) + 4.5).abs() < 1e-4);
    }

    #[test]
    fn pchip_stays_within_the_data() {
        let xs = [0.0, 1.0, 2.0, 3.0];
        let ys = [0.0, 10.0, 0.0, 0.0];
        for k in 0..=100 {
            let v = pchip(xs, ys, 1.0 + k as f32 / 100.0);
            assert!((0.0..=10.0).contains(&v), "{v}");
        }
    }
}
