//! Moves captured microphone audio onto the device timeline: an adaptive
//! sinc resampler (rubato) from the microphone's clock to the virtual
//! device's 48 kHz, steered by a PI loop so its output stays where the
//! capture time stamps say it belongs. Pure Rust; realtime-safe after
//! construction.
//!
//! Convention: the frame written at timeline position `q` carries the sound
//! captured at `q - R`, `R` being the resampler delay. A restarted resampler
//! emits `R` frames of pre-roll first, so anchoring its first output at the
//! timeline position of the first source frame, `P(Ts)` from its host time
//! stamp, gives exactly that. From then on the next output position should
//! stay at `P(Ts)` of each new block; the difference is the phase error the
//! loop removes by nudging the conversion ratio, never by dropping or
//! repeating samples. A discontinuity (a gap in the source sample times, or
//! an error beyond [`HARD_ERROR`]) re-anchors: the resampler restarts and
//! the output jumps to the right position, leaving a gap on the timeline.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Adjustable, Async, FixedAsync, Resampler, Resizable, SincInterpolationParameters,
    SincInterpolationType, WindowFunction,
};

/// Sinc taps: 0.67 ms of delay at 48 kHz.
const SINC_LEN: usize = 64;
/// The loop may move the ratio this far from nominal (1000 ppm).
const MAX_CORRECTION: f64 = 1e-3;
/// Phase error, frames, beyond which the converter re-anchors (2 ms).
pub const HARD_ERROR: f64 = 96.0;
/// PI gains for a critically damped loop with a 2 s time constant on a
/// 48 kHz timeline: natural frequency 0.5 rad/s.
const KP: f64 = 2.0 * 0.5 / 48_000.0;
const KI: f64 = 0.25 / 48_000.0;

/// What one conversion produced.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Converted {
    /// Timeline position of the first output frame.
    pub pos: i64,
    pub frames: usize,
    /// Phase error before this block, frames.
    pub error: f64,
    /// True when this block started a new anchor.
    pub anchored: bool,
}

pub struct Converter {
    rs: Async<f32>,
    nominal: f64,
    delay: f64,
    max_input: usize,
    /// Next output position; `None` before the first anchor.
    next: Option<i64>,
    /// Source sample time expected for the next block.
    expected: f64,
    integral: f64,
    /// Timeline position of the previous block, for the loop's time step.
    last_pos: f64,
    correction: f64,
    reanchors: u64,
}

impl Converter {
    /// A converter from `source_rate` to the 48 kHz timeline for source
    /// blocks of at most `max_input` frames. Allocates.
    pub fn new(source_rate: f64, max_input: usize) -> Self {
        let nominal = 48_000.0 / source_rate;
        let rs = Async::<f32>::new_sinc(
            nominal,
            1.0 + 2.0 * MAX_CORRECTION,
            &sinc_params(),
            max_input,
            1,
            FixedAsync::Input,
        )
        .expect("valid resampler parameters");
        Self {
            delay: rs.output_delay() as f64,
            rs,
            nominal,
            max_input,
            next: None,
            expected: 0.0,
            integral: 0.0,
            last_pos: 0.0,
            correction: 0.0,
            reanchors: 0,
        }
    }

    /// Output frames one block can produce at most: the output buffer size.
    pub fn max_output(&self) -> usize {
        self.rs.output_frames_max()
    }

    /// Resampler delay, frames on the timeline.
    pub fn delay(&self) -> f64 {
        self.delay
    }

    /// Current ratio correction (1e-6 is 1 ppm).
    pub fn correction(&self) -> f64 {
        self.correction
    }

    /// Times the converter re-anchored after the first anchor.
    pub fn reanchors(&self) -> u64 {
        self.reanchors
    }

    /// Converts one capture block. `sample_time` is the source sample time
    /// of its first frame, `position` the timeline position of that frame's
    /// host time. `out` holds at least [`Converter::max_output`] frames.
    /// Realtime-safe.
    pub fn process(
        &mut self,
        input: &[f32],
        sample_time: f64,
        position: f64,
        out: &mut [f32],
    ) -> Converted {
        let input = &input[..input.len().min(self.max_input)];
        let current = self
            .next
            .filter(|&n| sample_time == self.expected && (position - n as f64).abs() <= HARD_ERROR);
        let anchored = current.is_none();
        let pos = match current {
            Some(n) => n,
            None => {
                if self.next.is_some() {
                    self.reanchors += 1;
                }
                self.rs.reset();
                self.integral = 0.0;
                self.correction = 0.0;
                position.round() as i64
            }
        };
        let error = position - pos as f64;
        if !anchored {
            let dt = ((position - self.last_pos) / 48_000.0).clamp(0.0, 1.0);
            self.integral =
                (self.integral + error * dt).clamp(-MAX_CORRECTION / KI, MAX_CORRECTION / KI);
            self.correction =
                (KP * error + KI * self.integral).clamp(-MAX_CORRECTION, MAX_CORRECTION);
        }
        // Within bounds by construction; a refused ratio keeps the old one.
        let _ = self
            .rs
            .set_resample_ratio(self.nominal * (1.0 + self.correction), true);
        self.last_pos = position;
        self.expected = sample_time + input.len() as f64;

        let frames = if input.is_empty() {
            0
        } else {
            // Within 1..=max_input, the size the resampler was built for.
            let _ = self.rs.set_chunk_size(input.len());
            resample(&mut self.rs, input, out)
        };
        self.next = Some(pos + frames as i64);
        Converted {
            pos,
            frames,
            error,
            anchored,
        }
    }
}

/// The sinc filter both resamplers use: 64 taps, 0.67 ms at 48 kHz.
pub(crate) fn sinc_params() -> SincInterpolationParameters {
    SincInterpolationParameters {
        sinc_len: SINC_LEN,
        f_cutoff: None,
        oversampling_factor: 256,
        interpolation: SincInterpolationType::Linear,
        window: WindowFunction::BlackmanHarris2,
    }
}

/// Runs one mono block through `rs` into `out`, returning the frames
/// written. The adapters match their slices exactly, so only resampling
/// itself can fail, and then nothing is written.
pub(crate) fn resample(rs: &mut Async<f32>, input: &[f32], out: &mut [f32]) -> usize {
    let len = out.len();
    let i = InterleavedSlice::new(input, 1, input.len()).expect("mono, whole slice");
    let mut o = InterleavedSlice::new_mut(out, 1, len).expect("mono, whole slice");
    rs.process_into_buffer(&i, &mut o, None)
        .map_or(0, |(_, n)| n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `seconds` of a source running at `rate` (nominal `nominal`) in
    /// blocks of `block` frames; positions come from the true source clock.
    /// Returns the output on the timeline and the converter.
    fn run(
        nominal: f64,
        rate: f64,
        block: usize,
        seconds: f64,
    ) -> (Vec<(i64, f32)>, Vec<Converted>, Converter) {
        let mut c = Converter::new(nominal, 1024);
        let mut out = vec![0.0; c.max_output()];
        let mut written = Vec::new();
        let mut log = Vec::new();
        let total = (rate * seconds) as usize;
        let origin = 1_000_000.0;
        let mut t = 0usize;
        let mut input = vec![0.0f32; block];
        while t + block <= total {
            for (i, s) in input.iter_mut().enumerate() {
                *s = (std::f64::consts::TAU * 1000.0 * (t + i) as f64 / rate).sin() as f32 * 0.5;
            }
            let position = origin + t as f64 * 48_000.0 / rate;
            let r = c.process(&input, t as f64, position, &mut out);
            written.extend(
                out[..r.frames]
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| (r.pos + i as i64, v)),
            );
            log.push(r);
            t += block;
        }
        (written, log, c)
    }

    fn contiguous(w: &[(i64, f32)]) -> bool {
        w.windows(2).all(|p| p[1].0 == p[0].0 + 1)
    }

    #[test]
    fn tracks_a_fast_and_a_slow_source_without_gaps() {
        for ppm in [-200.0, 0.0, 200.0] {
            let rate = 48_000.0 * (1.0 + ppm * 1e-6);
            let (w, log, c) = run(48_000.0, rate, 64, 30.0);
            assert!(contiguous(&w), "{ppm} ppm: gap or overlap");
            assert_eq!(c.reanchors(), 0, "{ppm} ppm");
            // After settling, the phase error stays within the one-frame
            // sawtooth of whole output counts, and the correction matches
            // the drift.
            let late = &log[log.len() * 2 / 3..];
            let worst = late.iter().map(|r| r.error.abs()).fold(0.0, f64::max);
            assert!(worst <= 1.5, "{ppm} ppm: error {worst} frames");
            let expected = 1.0 / (1.0 + ppm * 1e-6) - 1.0;
            assert!(
                (c.correction() - expected).abs() < 10e-6,
                "{ppm} ppm: correction {} vs {expected}",
                c.correction()
            );
        }
    }

    #[test]
    fn converts_44_1_khz_at_the_right_rate() {
        let (w, log, c) = run(44_100.0, 44_100.0, 64, 10.0);
        assert!(contiguous(&w));
        assert_eq!(c.reanchors(), 0);
        assert!(log[log.len() / 2..].iter().all(|r| r.error.abs() <= 1.5));
        // 1 kHz in, 1 kHz out on the 48 kHz timeline.
        let tail: Vec<f32> = w[w.len() / 2..].iter().map(|p| p.1).collect();
        let rising = tail
            .windows(2)
            .filter(|p| p[0] < 0.0 && p[1] >= 0.0)
            .count() as f64;
        let hz = rising * 48_000.0 / tail.len() as f64;
        assert!((hz - 1000.0).abs() < 1.0, "{hz} Hz");
    }

    #[test]
    fn the_declared_delay_is_the_true_one() {
        for (rate, block) in [(48_000.0, 64usize), (48_000.0, 480), (44_100.0, 64)] {
            let mut c = Converter::new(rate, 1024);
            let mut out = vec![0.0; c.max_output()];
            let at = 20 * block + 10;
            let mut energy = (0.0f64, 0.0f64);
            let mut t = 0;
            while t < 20_000 {
                let mut input = vec![0.0f32; block];
                if (t..t + block).contains(&at) {
                    input[at - t] = 1.0;
                }
                let r = c.process(
                    &input,
                    t as f64,
                    1000.0 + t as f64 * 48_000.0 / rate,
                    &mut out,
                );
                for (i, v) in out[..r.frames].iter().enumerate() {
                    let w = f64::from(v * v);
                    energy.0 += w * (r.pos + i as i64) as f64;
                    energy.1 += w;
                }
                t += block;
            }
            // The impulse's energy centre lands at its own timeline position
            // plus the declared delay.
            let centre = energy.0 / energy.1;
            let expected = 1000.0 + at as f64 * 48_000.0 / rate + c.delay();
            assert!(
                (centre - expected).abs() < 1.0,
                "{rate} Hz, block {block}: centre {centre} vs {expected}"
            );
        }
    }

    #[test]
    fn a_source_gap_reanchors_once() {
        let mut c = Converter::new(48_000.0, 1024);
        let mut out = vec![0.0; c.max_output()];
        let input = [0.1f32; 64];
        let a = c.process(&input, 0.0, 5_000.0, &mut out);
        assert!(a.anchored);
        let b = c.process(&input, 64.0, 5_064.0, &mut out);
        assert!(!b.anchored);
        assert_eq!(b.pos, a.pos + a.frames as i64);
        // 1000 source frames went missing.
        let d = c.process(&input, 1_128.0, 6_128.0, &mut out);
        assert!(d.anchored);
        assert_eq!(c.reanchors(), 1);
        assert_eq!(d.pos, 6_128);
    }
}
