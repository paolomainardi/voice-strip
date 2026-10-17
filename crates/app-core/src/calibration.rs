//! First-run calibration: 5 s of silence, then 10 s of speech, from the raw
//! (pre-chain) input. Produces a [`VoiceProfile`]. Runs on a non-realtime
//! thread; it is fed from the engine's input scope queue.

use dsp::loudness::KWeighting;
use dsp::util::{Biquad, Coefs};

use crate::tuning::{CALIBRATED_LUFS, VoiceProfile};

const FS: usize = dsp::SAMPLE_RATE as usize;
pub const SILENCE_SECONDS: usize = 5;
pub const SPEECH_SECONDS: usize = 10;
const BLOCK: usize = FS * 4 / 10;
const HOP: usize = FS / 10;
const MAX_TRIM_DB: f32 = 30.0;
const MAX_EQ_CORRECTION_DB: f32 = 4.0;
const MIN_SNR_DB: f32 = 15.0;

/// Sibilance search: 1/6-octave bands from 4 to 10 kHz.
const SIB_BANDS: usize = 9;

/// Target voice curve: long-term level of each EQ band relative to the
/// full-band level, dB, measured with the band-pass detectors below.
/// Initial values for a balanced spoken voice; to be refined on the team's
/// licensed recordings.
pub const TARGET_CURVE_DB: [f32; 4] = [-7.0, -10.0, -15.0, -24.0];
const CURVE_Q: [f32; 4] = [1.0, 1.4, 1.8, 0.7];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Silence,
    Speech,
    Done,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CalibrationError {
    /// Speech never rose clearly above the noise floor.
    TooQuiet {
        speech_lufs: f32,
    },
    TooNoisy {
        snr_db: f32,
    },
}

impl std::fmt::Display for CalibrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooQuiet { speech_lufs } => {
                write!(
                    f,
                    "speech too quiet ({speech_lufs:.1} LUFS); move closer to the microphone"
                )
            }
            Self::TooNoisy { snr_db } => {
                write!(
                    f,
                    "background too noisy (SNR {snr_db:.1} dB); reduce noise and retry"
                )
            }
        }
    }
}

impl std::error::Error for CalibrationError {}

pub struct Calibrator {
    phase: Phase,
    count: usize,
    silence_sq: f64,
    kw: KWeighting,
    hop_acc: f64,
    hop_n: usize,
    hops: Vec<f64>,
    curve: [Biquad; 4],
    curve_e: [f64; 4],
    full_e: f64,
    sib: [Biquad; SIB_BANDS],
    sib_e: [f64; SIB_BANDS],
    result: Option<Result<VoiceProfile, CalibrationError>>,
}

impl Default for Calibrator {
    fn default() -> Self {
        Self::new()
    }
}

fn sib_freq(i: usize) -> f32 {
    4000.0 * 2f32.powf(i as f32 / 6.0)
}

impl Calibrator {
    pub fn new() -> Self {
        Self {
            phase: Phase::Silence,
            count: 0,
            silence_sq: 0.0,
            kw: KWeighting::new(),
            hop_acc: 0.0,
            hop_n: 0,
            hops: Vec::with_capacity(SPEECH_SECONDS * 10),
            curve: std::array::from_fn(|i| {
                Biquad::new(Coefs::bandpass(dsp::stages::dyneq::BAND_HZ[i], CURVE_Q[i]))
            }),
            curve_e: [0.0; 4],
            full_e: 0.0,
            sib: std::array::from_fn(|i| Biquad::new(Coefs::bandpass(sib_freq(i), 4.3))),
            sib_e: [0.0; SIB_BANDS],
            result: None,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Progress of the current phase, 0..=1.
    pub fn progress(&self) -> f32 {
        let len = match self.phase {
            Phase::Silence => SILENCE_SECONDS * FS,
            Phase::Speech => SPEECH_SECONDS * FS,
            Phase::Done => return 1.0,
        };
        self.count as f32 / len as f32
    }

    pub fn result(&self) -> Option<&Result<VoiceProfile, CalibrationError>> {
        self.result.as_ref()
    }

    pub fn feed(&mut self, samples: &[f32]) -> Phase {
        for &x in samples {
            match self.phase {
                Phase::Silence => {
                    self.silence_sq += (x as f64) * (x as f64);
                    self.count += 1;
                    if self.count == SILENCE_SECONDS * FS {
                        self.phase = Phase::Speech;
                        self.count = 0;
                    }
                }
                Phase::Speech => {
                    self.speech_sample(x);
                    self.count += 1;
                    if self.count == SPEECH_SECONDS * FS {
                        self.result = Some(self.analyze());
                        self.phase = Phase::Done;
                    }
                }
                Phase::Done => break,
            }
        }
        self.phase
    }

    fn speech_sample(&mut self, x: f32) {
        let k = self.kw.tick(x) as f64;
        self.hop_acc += k * k;
        self.hop_n += 1;
        if self.hop_n == HOP {
            self.hops.push(self.hop_acc / HOP as f64);
            self.hop_acc = 0.0;
            self.hop_n = 0;
        }
        let xd = x as f64;
        self.full_e += xd * xd;
        for (f, e) in self.curve.iter_mut().zip(self.curve_e.iter_mut()) {
            let y = f.tick(x) as f64;
            *e += y * y;
        }
        for (f, e) in self.sib.iter_mut().zip(self.sib_e.iter_mut()) {
            let y = f.tick(x) as f64;
            *e += y * y;
        }
    }

    fn analyze(&self) -> Result<VoiceProfile, CalibrationError> {
        let speech_lufs = gated_loudness(&self.hops);
        let noise_db = (10.0
            * (self.silence_sq / (SILENCE_SECONDS * FS) as f64)
                .max(1e-14)
                .log10()) as f32;
        if speech_lufs < -60.0 {
            return Err(CalibrationError::TooQuiet { speech_lufs });
        }
        let snr_db = speech_lufs - noise_db;
        if snr_db < MIN_SNR_DB {
            return Err(CalibrationError::TooNoisy { snr_db });
        }
        let trim_db = (CALIBRATED_LUFS - speech_lufs).clamp(-MAX_TRIM_DB, MAX_TRIM_DB);
        let (best, _) =
            self.sib_e.iter().enumerate().fold(
                (0, 0.0),
                |acc, (i, &e)| if e > acc.1 { (i, e) } else { acc },
            );
        let full = self.full_e.max(1e-20);
        let mut eq = [0.0; 4];
        for i in 0..4 {
            let rel = (10.0 * (self.curve_e[i] / full).max(1e-20).log10()) as f32;
            eq[i] = ((TARGET_CURVE_DB[i] - rel) * 0.5)
                .clamp(-MAX_EQ_CORRECTION_DB, MAX_EQ_CORRECTION_DB);
        }
        Ok(VoiceProfile {
            trim_db,
            noise_floor_db: noise_db + trim_db,
            sibilance_hz: sib_freq(best),
            eq_correction_db: eq,
        })
    }
}

/// BS.1770 gated integrated loudness from 100 ms hop energies (400 ms
/// blocks, 75 % overlap, -70 LUFS absolute and -10 LU relative gates).
fn gated_loudness(hops: &[f64]) -> f32 {
    let blocks: Vec<f64> = hops
        .windows(BLOCK / HOP)
        .map(|w| w.iter().sum::<f64>() / w.len() as f64)
        .collect();
    let lufs = |p: f64| -0.691 + 10.0 * p.max(1e-20).log10();
    let abs: Vec<f64> = blocks.into_iter().filter(|&p| lufs(p) > -70.0).collect();
    if abs.is_empty() {
        return -120.0;
    }
    let mean = abs.iter().sum::<f64>() / abs.len() as f64;
    let rel_gate = lufs(mean) - 10.0;
    let kept: Vec<f64> = abs.into_iter().filter(|&p| lufs(p) > rel_gate).collect();
    lufs(kept.iter().sum::<f64>() / kept.len() as f64) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 40) as f32 / (1u64 << 23) as f32 - 1.0
        }
    }

    fn speechy(n: usize, amp: f32, sib_hz: f32, rng: &mut Rng) -> Vec<f32> {
        let mut sib = Biquad::new(Coefs::bandpass(sib_hz, 3.0));
        let mut f1 = Biquad::new(Coefs::bandpass(600.0, 2.0));
        let mut f2 = Biquad::new(Coefs::bandpass(1800.0, 3.0));
        let mut phase = 0.0f32;
        (0..n)
            .map(|i| {
                let t = i as f32 / FS as f32;
                phase += 130.0 / FS as f32;
                let p = if phase >= 1.0 {
                    phase -= 1.0;
                    1.0
                } else {
                    0.0
                };
                let syll = (0.5 - 0.5 * (std::f32::consts::TAU * 4.0 * t).cos()).powi(2);
                let s = if (t % 1.0) > 0.8 {
                    sib.tick(rng.next()) * 0.4
                } else {
                    0.0
                };
                amp * 10.0 * (syll * (f1.tick(p) * 6.0 + f2.tick(p) * 3.0) + s)
            })
            .collect()
    }

    fn calibrate(amp: f32, noise: f32, sib_hz: f32) -> Result<VoiceProfile, CalibrationError> {
        let mut rng = Rng(5);
        let mut c = Calibrator::new();
        let silence: Vec<f32> = (0..SILENCE_SECONDS * FS)
            .map(|_| noise * rng.next())
            .collect();
        assert_eq!(c.feed(&silence), Phase::Speech);
        let mut sp = speechy(SPEECH_SECONDS * FS, amp, sib_hz, &mut rng);
        for v in sp.iter_mut() {
            *v += noise * rng.next();
        }
        assert_eq!(c.feed(&sp), Phase::Done);
        // Reference loudness for the trim check.
        c.result().unwrap().clone()
    }

    #[test]
    fn trim_brings_speech_to_calibrated_level() {
        let quiet = calibrate(0.02, 0.00005, 7000.0).unwrap();
        let loud = calibrate(0.2, 0.0005, 7000.0).unwrap();
        // 20 dB louder input needs 20 dB less trim.
        assert!(
            (quiet.trim_db - loud.trim_db - 20.0).abs() < 0.3,
            "{quiet:?} {loud:?}"
        );
        // Noise floor after trim is the same relative level.
        assert!((quiet.noise_floor_db - loud.noise_floor_db).abs() < 0.5);
    }

    #[test]
    fn detects_sibilance_frequency_within_a_band() {
        for f in [5000.0f32, 7000.0, 9000.0] {
            let p = calibrate(0.1, 0.0001, f).unwrap();
            let ratio = (p.sibilance_hz / f).log2().abs();
            assert!(
                ratio < 1.0 / 6.0,
                "{f} Hz detected as {} Hz",
                p.sibilance_hz
            );
        }
    }

    #[test]
    fn rejects_noisy_and_silent_rooms() {
        assert!(matches!(
            calibrate(0.01, 0.05, 7000.0),
            Err(CalibrationError::TooNoisy { .. })
        ));
        assert!(matches!(
            calibrate(0.0, 0.0, 7000.0),
            Err(CalibrationError::TooQuiet { .. })
        ));
    }

    #[test]
    fn eq_correction_is_bounded() {
        let p = calibrate(0.1, 0.0001, 7000.0).unwrap();
        for v in p.eq_correction_db {
            assert!(v.abs() <= MAX_EQ_CORRECTION_DB);
        }
    }

    #[test]
    fn gated_loudness_of_a_sine_is_bs1770() {
        // 1 kHz sine at -20 dBFS peak reads about -23.0 LUFS (+-0.1).
        let mut kw = KWeighting::new();
        let mut hops = Vec::new();
        let mut acc = 0.0;
        for i in 0..FS * 5 {
            let x = 0.1 * (std::f32::consts::TAU * 1000.0 * i as f32 / FS as f32).sin();
            let k = kw.tick(x) as f64;
            acc += k * k;
            if (i + 1) % HOP == 0 {
                hops.push(acc / HOP as f64);
                acc = 0.0;
            }
        }
        let l = gated_loudness(&hops);
        assert!((l + 23.0).abs() < 0.15, "{l}");
    }
}
