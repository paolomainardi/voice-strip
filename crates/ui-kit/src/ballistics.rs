//! Meter ballistics, time based (frame `dt` in seconds) so they behave the
//! same at any frame rate.

/// VU meter movement: a moving coil (mass, spring, damper) driven by the
/// level in the linear domain, tuned to ASA C16.5: 99 % of a step in
/// 300 ms with about 1.25 % overshoot, and the same on the way down.
/// Parameters from Lobdell and Allen, "A model of the VU (volume-unit)
/// meter, with speech applications", JASA 121 (2007).
///
/// Each update applies the exact solution for an input held over `dt`, so
/// the needle moves the same at any frame rate. The needle rests at zero
/// and stops at full scale, like a real movement against its pins.
#[derive(Clone, Copy, Debug)]
pub struct VuBallistics {
    /// Needle position as a linear level (0 dB = 1.0).
    level: f32,
    /// Needle speed, linear level per second.
    speed: f32,
}

impl VuBallistics {
    /// Damping ratio of the movement.
    pub const ZETA: f32 = 0.812_72;
    /// Natural frequency of the movement, rad/s.
    pub const OMEGA_N: f32 = 13.512;
    /// Full-scale stop: +3 VU.
    pub const FULL_SCALE_DB: f32 = 3.0;
    const FLOOR_DB: f32 = -60.0;

    pub fn new() -> Self {
        Self {
            level: 0.0,
            speed: 0.0,
        }
    }

    /// Feeds the latest block level (dB) and advances by `dt` seconds.
    pub fn update(&mut self, input_db: f32, dt: f32) -> f32 {
        let target = if input_db > Self::FLOOR_DB {
            db_to_lin(input_db)
        } else {
            0.0
        };
        let dt = dt.max(0.0);
        let sigma = Self::ZETA * Self::OMEGA_N;
        let omega_d = Self::OMEGA_N * (1.0 - Self::ZETA * Self::ZETA).sqrt();
        let e = self.level - target;
        let b = (self.speed + sigma * e) / omega_d;
        let decay = (-sigma * dt).exp();
        let (sin, cos) = (omega_d * dt).sin_cos();
        self.level = target + decay * (e * cos + b * sin);
        self.speed = decay * ((omega_d * b - sigma * e) * cos - (omega_d * e + sigma * b) * sin);
        let full = db_to_lin(Self::FULL_SCALE_DB);
        if self.level <= 0.0 {
            self.level = 0.0;
            self.speed = self.speed.max(0.0);
        } else if self.level >= full {
            self.level = full;
            self.speed = self.speed.min(0.0);
        }
        self.db()
    }

    /// Needle position in dB, at least -60.
    pub fn db(&self) -> f32 {
        lin_to_db(self.level).max(Self::FLOOR_DB)
    }

    /// Needle position as a linear level (0 dB = 1.0).
    pub fn level(&self) -> f32 {
        self.level
    }
}

impl Default for VuBallistics {
    fn default() -> Self {
        Self::new()
    }
}

/// Peak lamp: lights when the input exceeds the threshold and stays lit
/// for `hold_s`.
#[derive(Clone, Copy, Debug)]
pub struct PeakHold {
    threshold_db: f32,
    hold_s: f32,
    remaining: f32,
}

impl PeakHold {
    pub fn new(threshold_db: f32, hold_s: f32) -> Self {
        Self {
            threshold_db,
            hold_s,
            remaining: 0.0,
        }
    }

    pub fn update(&mut self, peak_db: f32, dt: f32) -> bool {
        if peak_db > self.threshold_db {
            self.remaining = self.hold_s;
        } else {
            self.remaining = (self.remaining - dt).max(0.0);
        }
        self.remaining > 0.0
    }

    pub fn lit(&self) -> bool {
        self.remaining > 0.0
    }
}

/// Gain-reduction display: instant attack, slow release so the LEDs do not
/// flicker between blocks.
#[derive(Clone, Copy, Debug, Default)]
pub struct GrSmoother {
    value: f32,
}

impl GrSmoother {
    pub const RELEASE_DB_PER_S: f32 = 40.0;

    pub fn update(&mut self, gr_db: f32, dt: f32) -> f32 {
        let gr = gr_db.max(0.0);
        if gr >= self.value {
            self.value = gr;
        } else {
            self.value = (self.value - Self::RELEASE_DB_PER_S * dt).max(gr);
        }
        self.value
    }

    pub fn value(&self) -> f32 {
        self.value
    }
}

/// Generic exponential smoother for bar meters (loudness, true peak).
#[derive(Clone, Copy, Debug)]
pub struct Smoother {
    value: f32,
    tau_s: f32,
}

impl Smoother {
    pub fn new(initial: f32, tau_s: f32) -> Self {
        Self {
            value: initial,
            tau_s,
        }
    }

    pub fn update(&mut self, target: f32, dt: f32) -> f32 {
        if !target.is_finite() {
            return self.value;
        }
        let k = 1.0 - (-dt / self.tau_s).exp();
        self.value += (target - self.value) * k.clamp(0.0, 1.0);
        self.value
    }

    pub fn value(&self) -> f32 {
        self.value
    }
}

pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

pub fn lin_to_db(lin: f32) -> f32 {
    if lin > 0.0 {
        20.0 * lin.log10()
    } else {
        f32::NEG_INFINITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulates a step from `from_db` to `to_db` with frame times from
    /// `dts` (repeated), for `seconds`; returns (time, linear level) pairs.
    fn step(from_db: f32, to_db: f32, dts: &[f32], seconds: f32) -> Vec<(f32, f32)> {
        let mut vu = VuBallistics::new();
        for _ in 0..600 {
            vu.update(from_db, 0.01);
        }
        let mut t = 0.0;
        let mut out = vec![(0.0, vu.level())];
        let mut i = 0;
        while t < seconds {
            let dt = dts[i % dts.len()];
            i += 1;
            vu.update(to_db, dt);
            t += dt;
            out.push((t, vu.level()));
        }
        out
    }

    /// Time the level first crosses `threshold`, interpolated between frames.
    fn crossing(trace: &[(f32, f32)], threshold: f32, rising: bool) -> f32 {
        for w in trace.windows(2) {
            let ((t0, a), (t1, b)) = (w[0], w[1]);
            let crossed = if rising {
                a < threshold && b >= threshold
            } else {
                a > threshold && b <= threshold
            };
            if crossed {
                return t0 + (threshold - a) / (b - a) * (t1 - t0);
            }
        }
        f32::NAN
    }

    /// Frame times: 24 to 240 fps, and an irregular sequence.
    fn frame_rates() -> Vec<Vec<f32>> {
        let mut v: Vec<Vec<f32>> = [24.0f32, 30.0, 60.0, 144.0, 240.0]
            .iter()
            .map(|fps| vec![1.0 / fps])
            .collect();
        let mut x = 12345u32;
        v.push(
            (0..97)
                .map(|_| {
                    x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    0.004 + (x >> 16) as f32 / 65536.0 * 0.04
                })
                .collect(),
        );
        v
    }

    #[test]
    fn vu_rise_meets_asa_c16_5() {
        for dts in frame_rates() {
            let trace = step(-80.0, 0.0, &dts, 1.5);
            let t99 = crossing(&trace, 0.99, true);
            assert!((0.27..=0.33).contains(&t99), "t99 = {t99} at {dts:?}");
            let peak = trace.iter().map(|p| p.1).fold(0.0, f32::max);
            let overshoot = (peak - 1.0) * 100.0;
            // Frames sample the peak, so a low frame rate can read a little
            // under the 1.25 % of the continuous movement.
            assert!(
                (1.0..=1.5).contains(&overshoot),
                "overshoot {overshoot} % at {dts:?}"
            );
        }
    }

    #[test]
    fn vu_falls_like_it_rises() {
        for dts in frame_rates() {
            let trace = step(0.0, -80.0, &dts, 1.5);
            let t = crossing(&trace, 0.01, false);
            assert!((0.27..=0.33).contains(&t), "fall to 1 % = {t} at {dts:?}");
            assert!(trace.iter().all(|p| p.1 >= 0.0), "below rest");
        }
    }

    #[test]
    fn vu_is_frame_rate_independent() {
        for dts in frame_rates() {
            let trace = step(-80.0, -6.0, &dts, 1.0);
            for &(t, level) in trace.iter().filter(|p| p.0 > 0.05) {
                // From rest with a constant input, one step of `t` is the
                // continuous movement at time `t`.
                let want = step(-80.0, -6.0, &[t], t)[1].1;
                // Compare in dB where the needle is readable.
                let diff = (lin_to_db(level) - lin_to_db(want)).abs();
                assert!(diff < 0.05 || want < 0.1, "t = {t}: {diff} dB at {dts:?}");
            }
        }
    }

    #[test]
    fn vu_stops_at_the_pins() {
        let mut vu = VuBallistics::new();
        let full = db_to_lin(VuBallistics::FULL_SCALE_DB);
        for _ in 0..120 {
            vu.update(10.0, 1.0 / 60.0);
            assert!(vu.level() <= full);
        }
        assert_eq!(vu.level(), full);
        // Leaves the end stop on the first frame after the level drops.
        vu.update(-80.0, 1.0 / 60.0);
        assert!(vu.level() < full);
        for _ in 0..120 {
            vu.update(-80.0, 1.0 / 60.0);
            assert!(vu.level() >= 0.0);
        }
        assert_eq!(vu.db(), -60.0);
    }

    #[test]
    fn vu_holds_still_on_a_low_sine() {
        // 60 Hz in 10 ms blocks: each block holds 0.6 cycles, so the block
        // readings wobble; the movement must not.
        let amp = 10f32.powf(-18.0 / 20.0) * std::f32::consts::SQRT_2;
        let mut vu = VuBallistics::new();
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for b in 0..300 {
            let block: Vec<f32> = (0..480)
                .map(|i| {
                    let n = (b * 480 + i) as f32;
                    amp * (std::f32::consts::TAU * 60.0 * n / 48_000.0).sin()
                })
                .collect();
            let db = 20.0 * engine::meters::rectified_average(&block).log10();
            let needle = vu.update(db + 18.0, 0.01);
            if b >= 200 {
                lo = lo.min(needle);
                hi = hi.max(needle);
            }
        }
        assert!(hi - lo < 0.1, "ripple {} dB", hi - lo);
        assert!(lo.abs() < 0.5 && hi.abs() < 0.5, "reads {lo}..{hi} VU");
    }

    #[test]
    fn peak_hold_keeps_lamp_lit() {
        let mut p = PeakHold::new(0.0, 0.5);
        assert!(p.update(1.0, 0.016));
        assert!(p.update(-20.0, 0.3));
        assert!(!p.update(-20.0, 0.3));
    }

    #[test]
    fn gr_releases_slowly() {
        let mut g = GrSmoother::default();
        assert_eq!(g.update(6.0, 0.016), 6.0);
        let v = g.update(0.0, 0.05);
        assert!(v > 3.0 && v < 6.0);
    }
}
