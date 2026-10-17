//! Measurements for the neural denoiser: inference time, realtime operation
//! without misses or allocations, declared latency, and noise reduction.
//! Results are printed with `--nocapture` and recorded in docs/denoise.md.

use std::f32::consts::TAU;
use std::time::{Duration, Instant};

#[cfg(debug_assertions)]
use assert_no_alloc::AllocDisabler;
use assert_no_alloc::assert_no_alloc;
use denoise_neural::DfnDenoiser;
use df::tract::{DfParams, DfTract, RuntimeParams};
use dsp::{Chain, ChainSettings, Module, PathKind};

#[cfg(debug_assertions)]
#[global_allocator]
static A: AllocDisabler = AllocDisabler;

const FS: f32 = 48_000.0;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    }
}

/// Speech-like voiced signal: pulse train with pitch drift through three
/// formants, 4 Hz syllables, a 1.2 s pause every 4 s. Returns the signal and
/// a per-sample "voice active" mask.
fn voice(seconds: f32) -> (Vec<f32>, Vec<bool>) {
    let n = (seconds * FS) as usize;
    let mut out = Vec::with_capacity(n);
    let mut mask = Vec::with_capacity(n);
    let mut f: Vec<dsp::util::Biquad> = [(700.0, 4.0), (1200.0, 5.0), (2600.0, 6.0)]
        .iter()
        .map(|&(hz, q)| dsp::util::Biquad::new(dsp::util::Coefs::bandpass(hz, q)))
        .collect();
    let mut phase = 0.0f32;
    for i in 0..n {
        let t = i as f32 / FS;
        phase += (120.0 + 20.0 * (t * 1.3).sin()) / FS;
        let p = if phase >= 1.0 {
            phase -= 1.0;
            1.0
        } else {
            0.0
        };
        let active = (t % 4.0) < 2.8;
        let syll = (0.5 - 0.5 * (TAU * 4.0 * t).cos()).powf(0.7);
        let v = f[0].tick(p) * 3.0 + f[1].tick(p) * 2.0 + f[2].tick(p);
        out.push(if active { 0.5 * syll * v } else { 0.0 });
        mask.push(active);
    }
    (out, mask)
}

/// Fan-like noise: low-passed white noise plus a little hum.
fn noise(n: usize, amp: f32) -> Vec<f32> {
    let mut r = Rng(42);
    let mut lp = dsp::util::Biquad::new(dsp::util::Coefs::lowpass(3000.0, 0.7));
    (0..n)
        .map(|i| amp * (lp.tick(r.next()) + 0.2 * (TAU * 100.0 * i as f32 / FS).sin()))
        .collect()
}

fn rms_db(x: &[f32]) -> f32 {
    let ms = x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len().max(1) as f64;
    (10.0 * ms.max(1e-30).log10()) as f32
}

fn settings() -> ChainSettings {
    let mut enabled = [false; 9];
    enabled[Module::Denoise.index()] = true;
    ChainSettings {
        enabled,
        modules: dsp::ModuleSettings {
            denoise_mix: 1.0,
            denoise_atten_limit_db: 25.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// Inference time per 10 ms frame stays far below the frame period:
/// p99 under 2 ms.
#[test]
#[ignore = "timing-sensitive: run `just test-denoise` on an idle machine"]
fn inference_time_per_frame() {
    let mut m = DfTract::new(DfParams::default(), &RuntimeParams::default_with_ch(1)).unwrap();
    let hop = m.hop_size;
    let (v, _) = voice(12.0);
    let nz = noise(v.len(), 0.05);
    let mut noisy = ndarray::Array2::<f32>::zeros((1, hop));
    let mut enh = ndarray::Array2::<f32>::zeros((1, hop));
    let mut times = Vec::new();
    for (k, frame) in v.chunks_exact(hop).enumerate() {
        for (j, d) in noisy.iter_mut().enumerate() {
            *d = frame[j] + nz[k * hop + j];
        }
        let t = Instant::now();
        m.process(noisy.view(), enh.view_mut()).unwrap();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let max = times.iter().cloned().fold(0.0, f64::max);
    let p50 = percentile(&mut times, 0.5);
    let p99 = percentile(&mut times, 0.99);
    eprintln!(
        "dfn: frame {hop} samples, fft {}, lookahead {}, inference p50 {p50:.3} ms, p99 {p99:.3} ms, max {max:.3} ms",
        m.fft_size, m.lookahead
    );
    assert!(p99 < 2.0, "p99 {p99:.3} ms");
}

/// Runs `x` through a main chain with the neural denoiser, one 480-sample
/// block every 10 ms like an audio callback. Returns output, chain latency,
/// missed frames and the slowest audio-side block.
fn run_realtime(x: &[f32]) -> (Vec<f32>, usize, u64, Duration) {
    let mut chain = Chain::with_denoiser(
        PathKind::Main,
        480,
        Some(Box::new(DfnDenoiser::spawn().unwrap())),
    );
    chain.set_settings(&settings());
    let lat = chain.latency_samples();
    let mut y = x.to_vec();
    let period = Duration::from_millis(10);
    let mut next = Instant::now();
    let mut worst = Duration::ZERO;
    for b in y.chunks_mut(480) {
        let t = Instant::now();
        assert_no_alloc(|| {
            chain.process(b);
        });
        worst = worst.max(t.elapsed());
        next += period;
        if let Some(d) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
    }
    (y, lat, chain.denoise_missed_frames(), worst)
}

/// Realtime operation: blocks paced at 10 ms, the worker never misses a
/// frame, the audio side never allocates and stays far below its period.
/// Needs an otherwise idle machine (the worker is not yet RT-scheduled).
#[test]
#[ignore = "timing-sensitive: run `just test-denoise` on an idle machine"]
fn realtime_no_misses() {
    let (v, _) = voice(5.0);
    let nz = noise(v.len(), 0.02);
    let x: Vec<f32> = v.iter().zip(&nz).map(|(a, b)| a + b).collect();
    let (_, lat, misses, worst) = run_realtime(&x);
    eprintln!(
        "dfn: stage latency {lat} samples ({:.1} ms), missed frames {misses}, worst audio-side block {:.3} ms",
        lat as f32 / 48.0,
        worst.as_secs_f64() * 1000.0
    );
    assert_eq!(misses, 0);
    assert!(worst < Duration::from_millis(2));
}

/// Real speech (LibriSpeech, fetched by scripts/fetch-testdata.sh) with fan
/// noise at about 10 dB SNR: noise in pauses drops by the 25 dB attenuation
/// limit (+-3 dB),
/// speech level stays within 2 dB, and the measured delay (cross-correlation
/// with the clean speech) equals the declared latency within 2 samples.
#[test]
#[ignore = "needs testdata/speech (run scripts/fetch-testdata.sh)"]
fn real_speech_quality_and_latency() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/speech");
    let gap = 72_000;
    let mut clean = vec![0.0f32; gap];
    let mut mask = vec![false; gap];
    for i in 0..4 {
        let r = hound::WavReader::open(dir.join(format!("1272-128104-000{i}.wav")))
            .expect("missing testdata: run scripts/fetch-testdata.sh");
        let utt: Vec<f32> = r.into_samples::<f32>().map(|s| s.unwrap()).collect();
        mask.extend(std::iter::repeat_n(true, utt.len()));
        clean.extend(utt);
        clean.extend(std::iter::repeat_n(0.0, gap));
        mask.extend(std::iter::repeat_n(false, gap));
    }
    let speech_db = rms_db(&clean);
    let nz = noise(clean.len(), 1.0);
    let g = 10f32.powf((speech_db - 10.0 - rms_db(&nz)) / 20.0);
    let x: Vec<f32> = clean.iter().zip(&nz).map(|(a, b)| a + g * b).collect();
    let (y, lat, misses, _) = run_realtime(&x);
    let (mut p_in, mut p_out, mut s_in, mut s_out) = (vec![], vec![], vec![], vec![]);
    for (i, &out) in y.iter().enumerate().skip(lat + 24_000) {
        let j = i - lat;
        let lo = j.saturating_sub(9_600);
        let hi = (j + 9_600).min(mask.len() - 1);
        if mask[lo] == mask[j] && mask[hi] == mask[j] {
            if mask[j] {
                s_in.push(clean[j]);
                s_out.push(out);
            } else {
                p_in.push(x[j]);
                p_out.push(out);
            }
        }
    }
    let pause = rms_db(&p_out) - rms_db(&p_in);
    let speech = rms_db(&s_out) - rms_db(&s_in);
    let seg = 48_000..clean.len() - 48_000 - lat - 100;
    let mut best = (0usize, f64::MIN);
    for lag in lat - 100..lat + 100 {
        let c: f64 = seg
            .clone()
            .map(|i| clean[i] as f64 * y[i + lag] as f64)
            .sum();
        if c > best.1 {
            best = (lag, c);
        }
    }
    eprintln!(
        "dfn real speech: pause change {pause:.1} dB, speech change {speech:.1} dB, declared latency {lat}, measured {}, missed frames {misses}",
        best.0
    );
    write_wav("dfn_speech_in.wav", &x);
    write_wav("dfn_speech_out.wav", &y);
    assert_eq!(misses, 0);
    assert!((pause + 25.0).abs() < 3.0, "pause change {pause} dB");
    assert!(speech.abs() < 2.0, "speech change {speech} dB");
    assert!(
        (best.0 as i64 - lat as i64).abs() <= 2,
        "measured {}",
        best.0
    );
}

fn write_wav(name: &str, x: &[f32]) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/dsp-wav");
    std::fs::create_dir_all(&dir).unwrap();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(dir.join(name), spec).unwrap();
    for &v in x {
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
}

/// With every core saturated by busy threads, the realtime-priority worker
/// still delivers every frame.
#[test]
#[ignore = "saturates all CPU cores for a few seconds"]
fn realtime_under_cpu_load() {
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    let hogs: Vec<_> = (0..cores * 2)
        .map(|_| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut x = 0u64;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                    std::hint::black_box(x);
                }
            })
        })
        .collect();
    let d = DfnDenoiser::spawn().unwrap();
    let priority = d.priority();
    drop(d);
    let (v, _) = voice(5.0);
    let nz = noise(v.len(), 0.02);
    let x: Vec<f32> = v.iter().zip(&nz).map(|(a, b)| a + b).collect();
    let (_, _, misses, worst) = run_realtime(&x);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    for h in hogs {
        h.join().unwrap();
    }
    eprintln!(
        "dfn under load ({} busy threads): worker priority {priority:?}, missed frames {misses}, worst audio-side block {:.3} ms",
        cores * 2,
        worst.as_secs_f64() * 1000.0
    );
    assert_eq!(misses, 0);
}
