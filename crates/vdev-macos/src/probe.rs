//! Checks of captured test signals.
//!
//! - A position-coded signal, checked frame by frame: every captured frame
//!   must be either silence or exactly the frame written `latency` frames
//!   earlier on the device timeline, so torn, stale, repeated or missing
//!   frames and the true transport latency all show.
//! - A sine, for paths that resample: continuity and frequency.

/// Positions repeat every `CODE_SPAN` frames (21.8 s at 48 kHz).
pub const CODE_SPAN: i64 = 1 << 20;

/// The sample value written at timeline position `pos`: never zero, so
/// silence stays distinguishable, and exact in f32.
pub fn code(pos: i64) -> f32 {
    (pos.rem_euclid(CODE_SPAN) + 1) as f32 / (2 * CODE_SPAN) as f32
}

/// The position (modulo `CODE_SPAN`) a sample value encodes, if it is one.
pub fn decode(v: f32) -> Option<i64> {
    let n = f64::from(v) * (2 * CODE_SPAN) as f64;
    let r = n.round();
    ((n - r).abs() < 1e-3 && (1.0..=CODE_SPAN as f64).contains(&r)).then_some(r as i64 - 1)
}

/// One IO cycle of a capture: where it starts on the device timeline and how
/// many frames it holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Block {
    pub sample_time: f64,
    pub frames: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub blocks: usize,
    pub frames: usize,
    /// Blocks that do not start where the previous one ended.
    pub timeline_gaps: usize,
    /// Frames delivered as silence.
    pub silent: usize,
    /// Frames that are neither silence nor the frame `latency` earlier.
    pub wrong: usize,
    /// The latency the first coded frame shows, frames.
    pub measured_latency: Option<i64>,
}

/// Checks a capture against the expected transport `latency`.
pub fn analyze(samples: &[f32], blocks: &[Block], latency: i64) -> Report {
    let mut r = Report {
        blocks: blocks.len(),
        frames: samples.len(),
        timeline_gaps: blocks
            .windows(2)
            .filter(|w| w[1].sample_time != w[0].sample_time + f64::from(w[0].frames))
            .count(),
        ..Default::default()
    };
    let mut at = 0usize;
    for b in blocks {
        let start = b.sample_time as i64;
        for i in 0..b.frames as usize {
            let Some(&v) = samples.get(at + i) else {
                return r;
            };
            let pos = start + i as i64;
            if v == 0.0 {
                r.silent += 1;
                continue;
            }
            let shown = decode(v).map(|p| (pos - p).rem_euclid(CODE_SPAN));
            if r.measured_latency.is_none() {
                r.measured_latency = shown;
            }
            if shown != Some(latency) {
                r.wrong += 1;
            }
        }
        at += b.frames as usize;
    }
    r
}

/// Continuity of a captured sine: timeline gaps, silent frames, and the
/// largest sample-to-sample step against the largest a clean sine of this
/// frequency and peak can make (above 1 means a discontinuity).
#[derive(Clone, Debug, PartialEq)]
pub struct SineReport {
    pub frames: usize,
    pub timeline_gaps: usize,
    pub silent: usize,
    pub frequency_hz: f64,
    pub peak: f32,
    pub step_ratio: f64,
}

pub fn sine(samples: &[f32], blocks: &[Block], frequency_hz: f64) -> SineReport {
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let crossings: Vec<f64> = samples
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
        .map(|(i, w)| i as f64 + f64::from(-w[0] / (w[1] - w[0])))
        .collect();
    let measured = match (crossings.first(), crossings.last()) {
        (Some(a), Some(b)) if b > a => (crossings.len() - 1) as f64 * 48_000.0 / (b - a),
        _ => 0.0,
    };
    let max_step = samples
        .windows(2)
        .map(|w| f64::from((w[1] - w[0]).abs()))
        .fold(0.0, f64::max);
    let sine_step = f64::from(peak) * std::f64::consts::TAU * frequency_hz / 48_000.0;
    SineReport {
        frames: samples.len(),
        timeline_gaps: blocks
            .windows(2)
            .filter(|w| w[1].sample_time != w[0].sample_time + f64::from(w[0].frames))
            .count(),
        silent: samples.iter().filter(|v| **v == 0.0).count(),
        frequency_hz: measured,
        peak,
        step_ratio: if sine_step > 0.0 {
            max_step / sine_step
        } else {
            f64::INFINITY
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_sine_has_no_discontinuity_and_a_cut_one_does() {
        let s: Vec<f32> = (0..48_000)
            .map(|i| 0.25 * (std::f64::consts::TAU * 440.0 * i as f64 / 48_000.0).sin() as f32)
            .collect();
        let b = [Block {
            sample_time: 0.0,
            frames: 48_000,
        }];
        let r = sine(&s, &b, 440.0);
        assert!(
            r.step_ratio <= 1.001 && (r.frequency_hz - 440.0).abs() < 0.01,
            "{r:?}"
        );
        let mut cut = s[..20_000].to_vec();
        cut.extend_from_slice(&s[20_050..]);
        assert!(sine(&cut, &b, 440.0).step_ratio > 2.0);
    }

    #[test]
    fn codes_round_trip_and_are_never_silence() {
        for pos in [0, 1, 47_999, CODE_SPAN - 1, CODE_SPAN, 123_456_789] {
            assert_ne!(code(pos), 0.0);
            assert_eq!(decode(code(pos)), Some(pos.rem_euclid(CODE_SPAN)));
        }
        assert_eq!(decode(0.3), None);
    }

    fn capture(starts: &[i64], frames: u32, latency: i64) -> (Vec<f32>, Vec<Block>) {
        let samples = starts
            .iter()
            .flat_map(|&s| (s..s + i64::from(frames)).map(move |p| code(p - latency)))
            .collect();
        let blocks = starts
            .iter()
            .map(|&s| Block {
                sample_time: s as f64,
                frames,
            })
            .collect();
        (samples, blocks)
    }

    #[test]
    fn a_clean_capture_shows_its_latency() {
        let (s, b) = capture(&[1000, 1512, 2024], 512, 128);
        let r = analyze(&s, &b, 128);
        assert_eq!((r.timeline_gaps, r.silent, r.wrong), (0, 0, 0));
        assert_eq!(r.measured_latency, Some(128));
    }

    #[test]
    fn stale_silent_and_skipped_frames_are_counted() {
        let (mut s, b) = capture(&[1000, 1512, 3000], 512, 128);
        s[10] = 0.0;
        s[20] = code(5);
        let r = analyze(&s, &b, 128);
        assert_eq!((r.timeline_gaps, r.silent, r.wrong), (1, 1, 1));
    }
}
