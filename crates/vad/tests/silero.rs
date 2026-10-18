//! Silero VAD acceptance: silence and steady noise read as no speech,
//! recorded speech reads as speech, and pushing samples never allocates.

#[cfg(debug_assertions)]
use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use dsp::VoiceDetector;
use vad::SileroVad;

#[cfg(debug_assertions)]
#[global_allocator]
static A: AllocDisabler = AllocDisabler;

const BLOCK: usize = 480;

/// Feeds `x` ten times faster than real time and samples the probability
/// after every block.
fn probabilities(vad: &mut SileroVad, x: &[f32]) -> Vec<f32> {
    x.chunks(BLOCK)
        .map(|b| {
            vad.push(b);
            std::thread::sleep(std::time::Duration::from_millis(1));
            vad.speech_probability().unwrap_or(0.0)
        })
        .collect()
}

fn noise(n: usize, rms: f32, mut seed: u64) -> Vec<f32> {
    (0..n)
        .map(|_| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * rms * 3.46
        })
        .collect()
}

/// Two seconds of silence, then of steady noise at -45 dBFS: after the first
/// half second the probability stays below 0.35 (no speech).
#[test]
fn silence_and_noise_are_not_speech() {
    let mut vad = SileroVad::spawn().expect("model loads");
    let mut x = vec![0.0; 96_000];
    x.extend(noise(96_000, 10f32.powf(-45.0 / 20.0), 7));
    let p = probabilities(&mut vad, &x);
    let max = p[50..].iter().copied().fold(0.0, f32::max);
    assert!(max < 0.35, "max probability {max}");
    assert!(vad.inference_ms().is_some_and(|ms| ms < 5.0));
}

/// Recorded speech separated by 1.5 s of noise at -50 dBFS: at least 80% of
/// the speech blocks read speech (>= 0.5) and at least 90% of the blocks in
/// the middle second of each pause read no speech (< 0.35).
#[test]
#[ignore = "needs testdata/speech (run scripts/fetch-testdata.sh)"]
fn recorded_speech_is_speech() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/speech");
    let mut x = Vec::new();
    let mut speech = Vec::new();
    for i in 0..4 {
        let r = hound::WavReader::open(dir.join(format!("1272-128104-000{i}.wav")))
            .expect("missing testdata: run scripts/fetch-testdata.sh");
        let s: Vec<f32> = r.into_samples::<f32>().map(|s| s.unwrap()).collect();
        speech.push((x.len(), x.len() + s.len()));
        x.extend(s);
        x.extend(noise(72_000, 10f32.powf(-50.0 / 20.0), i));
    }
    let mut vad = SileroVad::spawn().expect("model loads");
    let p = probabilities(&mut vad, &x);
    let block_in = |a: usize, b: usize| (a / BLOCK..b / BLOCK).map(|i| p[i]).collect::<Vec<_>>();
    let mut voiced = Vec::new();
    let mut pauses = Vec::new();
    for &(a, b) in &speech {
        voiced.extend(block_in(a, b));
        pauses.extend(block_in(b + 24_000, b + 72_000));
    }
    let share =
        |v: &[f32], f: fn(f32) -> bool| v.iter().filter(|&&p| f(p)).count() as f32 / v.len() as f32;
    let hit = share(&voiced, |p| p >= 0.5);
    let quiet = share(&pauses, |p| p < 0.35);
    assert!(
        hit >= 0.8,
        "speech detected in {:.0}% of speech",
        hit * 100.0
    );
    assert!(quiet >= 0.9, "no speech in {:.0}% of pauses", quiet * 100.0);
}

/// Pushing samples never allocates, including when the queue is full.
#[cfg(debug_assertions)]
#[test]
fn push_does_not_allocate() {
    let mut vad = SileroVad::spawn().expect("model loads");
    let block = [0.01f32; BLOCK];
    assert_no_alloc(|| {
        for _ in 0..200 {
            vad.push(&block);
            let _ = vad.speech_probability();
            let _ = vad.inference_ms();
        }
    });
}
