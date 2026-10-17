//! Headphone monitor output: the engine's monitor samples arrive from the
//! microphone's IOProc through a lock-free queue, on the 48 kHz device
//! timeline, and leave through the headphones' IOProc on the headphones' own
//! clock. An adaptive sinc resampler (rubato, fixed output size) bridges the
//! two clocks; a slow PI loop on the queue's fill level keeps it near a
//! target, so drift never empties or overfills it. Pure Rust; realtime-safe
//! after construction.

use rubato::{Adjustable, Async, FixedAsync, Resampler, Resizable};

use crate::convert::{resample, sinc_params};

/// Frames to keep queued between the two IOProcs for bursts of `burst`
/// frames (the larger of the microphone's and the headphones' buffers at
/// 48 kHz): bursts arrive and leave out of phase, so the queue swings by
/// about one burst around its level and must never drop below one.
pub fn target_fill(burst: usize) -> f64 {
    2.0 * burst as f64
}
const MAX_CORRECTION: f64 = 1e-3;
/// Critically damped PI with a 5 s time constant: the fill level saws by a
/// burst every callback, so the loop must be slower than that noise.
const KP: f64 = 2.0 * 0.2 / 48_000.0;
const KI: f64 = 0.04 / 48_000.0;
/// Time constant of the measured fill level's smoothing, seconds: long
/// against the callbacks' sawtooth, short against the loop.
const FILL_SMOOTHING_S: f64 = 0.1;

pub struct MonitorOut {
    rx: rtrb::Consumer<f32>,
    rs: Async<f32>,
    nominal: f64,
    max_output: usize,
    staging: Vec<f32>,
    target: f64,
    fill: f64,
    integral: f64,
    correction: f64,
    /// False until the queue holds the target again (start, underrun).
    playing: bool,
    underruns: u64,
}

impl MonitorOut {
    /// A monitor reading `rx`, keeping `target` frames queued, and rendering
    /// at `output_rate` in blocks of at most `max_output` frames. Allocates.
    pub fn new(rx: rtrb::Consumer<f32>, target: f64, output_rate: f64, max_output: usize) -> Self {
        let nominal = output_rate / 48_000.0;
        let rs = Async::<f32>::new_sinc(
            nominal,
            1.0 + 2.0 * MAX_CORRECTION,
            &sinc_params(),
            max_output,
            1,
            FixedAsync::Output,
        )
        .expect("valid resampler parameters");
        Self {
            staging: vec![0.0; rs.input_frames_max()],
            rx,
            rs,
            nominal,
            max_output,
            target,
            fill: 0.0,
            integral: 0.0,
            correction: 0.0,
            playing: false,
            underruns: 0,
        }
    }

    /// Gives the queue back, for the next headphones.
    pub fn into_queue(self) -> rtrb::Consumer<f32> {
        self.rx
    }

    /// Resampler delay, frames at the output rate.
    pub fn delay(&self) -> usize {
        self.rs.output_delay()
    }

    pub fn underruns(&self) -> u64 {
        self.underruns
    }

    /// Current ratio correction (1e-6 is 1 ppm).
    pub fn correction(&self) -> f64 {
        self.correction
    }

    /// Smoothed queue fill level, frames.
    pub fn fill(&self) -> f64 {
        self.fill
    }

    /// Fills `out` with the next mono frames, `dt` seconds after the
    /// previous call. Silence until the queue holds the target, and after an
    /// underrun until it does again. Realtime-safe.
    pub fn render(&mut self, out: &mut [f32], dt: f64) {
        let frames = out.len().min(self.max_output);
        let out = &mut out[..frames];
        if out.is_empty() {
            return;
        }
        // Within 1..=max_output, the size the resampler was built for.
        let _ = self.rs.set_chunk_size(out.len());
        let needed = self.rs.input_frames_next();
        let available = self.rx.slots();
        if !self.playing {
            if (available as f64) < self.target + needed as f64 {
                out.fill(0.0);
                return;
            }
            self.playing = true;
            self.fill = self.target;
            self.integral = 0.0;
            self.rs.reset();
        }
        if available < needed || needed > self.staging.len() {
            out.fill(0.0);
            self.underruns += 1;
            self.playing = false;
            return;
        }
        if let Ok(chunk) = self.rx.read_chunk(needed) {
            let (a, b) = chunk.as_slices();
            self.staging[..a.len()].copy_from_slice(a);
            self.staging[a.len()..needed].copy_from_slice(b);
            chunk.commit_all();
        }
        let produced = resample(&mut self.rs, &self.staging[..needed], out);
        out[produced..].fill(0.0);

        // Too much queued: consume faster, a lower output/input ratio.
        let dt = dt.clamp(0.0, 1.0);
        self.fill += (self.rx.slots() as f64 - self.fill) * (dt / FILL_SMOOTHING_S).min(1.0);
        let error = self.fill - self.target;
        self.integral =
            (self.integral + error * dt).clamp(-MAX_CORRECTION / KI, MAX_CORRECTION / KI);
        self.correction = (KP * error + KI * self.integral).clamp(-MAX_CORRECTION, MAX_CORRECTION);
        let _ = self
            .rs
            .set_resample_ratio(self.nominal * (1.0 - self.correction), true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The backend's IO buffer.
    const BLOCK: usize = 32;

    /// Simulates `seconds` of a producer pushing `BLOCK`-frame blocks at
    /// `in_rate` and the monitor rendering `BLOCK`-frame blocks at `out_rate`,
    /// interleaved by time. Returns the monitor and the rendered audio.
    /// Also returns the correction and fill level after every render in the
    /// second half.
    fn run(
        in_rate: f64,
        out_rate: f64,
        nominal_out: f64,
        seconds: f64,
    ) -> (MonitorOut, Vec<f32>, Vec<(f64, f64)>) {
        let (mut tx, rx) = rtrb::RingBuffer::new(4096);
        let mut m = MonitorOut::new(rx, target_fill(BLOCK), nominal_out, 512);
        let mut out = vec![0.0f32; BLOCK];
        let mut rendered = Vec::new();
        let mut late = Vec::new();
        let (mut t_in, mut t_out, mut n_in) = (0.0, 0.0, 0usize);
        let in_period = BLOCK as f64 / in_rate;
        let out_period = BLOCK as f64 / out_rate;
        while t_out < seconds {
            if t_in <= t_out {
                for i in 0..BLOCK {
                    let v =
                        (std::f64::consts::TAU * 440.0 * (n_in + i) as f64 / 48_000.0).sin() as f32;
                    let _ = tx.push(0.25 * v);
                }
                n_in += BLOCK;
                t_in += in_period;
            } else {
                m.render(&mut out, out_period);
                rendered.extend_from_slice(&out);
                if t_out > seconds / 2.0 {
                    late.push((m.correction(), m.fill()));
                }
                t_out += out_period;
            }
        }
        (m, rendered, late)
    }

    #[test]
    fn follows_clock_drift_without_underruns() {
        for ppm in [-300.0, 0.0, 300.0] {
            let in_rate = 48_000.0 * (1.0 + ppm * 1e-6);
            let (m, _, late) = run(in_rate, 48_000.0, 48_000.0, 120.0);
            assert_eq!(m.underruns(), 0, "{ppm} ppm");
            // On average the correction matches the drift; whole-frame
            // consumption makes it cycle slowly around that by well under
            // a cent of pitch.
            let mean = late.iter().map(|l| l.0).sum::<f64>() / late.len() as f64;
            assert!(
                (mean - ppm * 1e-6).abs() < 20e-6,
                "{ppm} ppm: mean correction {mean}"
            );
            let swing = late.iter().map(|l| (l.0 - mean).abs()).fold(0.0, f64::max);
            assert!(swing < 200e-6, "{ppm} ppm: correction swings {swing}");
            let target = target_fill(BLOCK);
            assert!(
                late.iter().all(|l| (l.1 - target).abs() < target / 2.0),
                "{ppm} ppm: fill leaves the target band"
            );
        }
    }

    #[test]
    fn renders_a_44_1_khz_output_at_the_right_pitch() {
        let (m, out, _) = run(48_000.0, 44_100.0, 44_100.0, 10.0);
        assert_eq!(m.underruns(), 0);
        let tail = &out[out.len() / 2..];
        let rising = tail
            .windows(2)
            .filter(|p| p[0] < 0.0 && p[1] >= 0.0)
            .count() as f64;
        let hz = rising * 44_100.0 / tail.len() as f64;
        assert!((hz - 440.0).abs() < 1.0, "{hz} Hz");
    }

    #[test]
    fn silence_until_the_queue_is_filled() {
        let (mut tx, rx) = rtrb::RingBuffer::new(4096);
        let mut m = MonitorOut::new(rx, target_fill(64), 48_000.0, 512);
        let mut out = vec![1.0f32; 64];
        for _ in 0..64 {
            let _ = tx.push(0.5);
        }
        m.render(&mut out, 0.001);
        assert!(out.iter().all(|v| *v == 0.0));
        assert_eq!(m.underruns(), 0);
    }
}
