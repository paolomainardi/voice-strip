//! Tonal balance independent of the microphone: the same voice through two
//! very different simulated microphones, each calibrated, ends up much closer
//! in long-term spectral balance after processing than before.

use app_core::calibration::{Calibrator, Phase, TARGET_CURVE_DB};
use app_core::{PresetId, settings_for};
use dsp::util::{Biquad, Coefs};
use dsp::{Chain, ChainSettings, PathKind};

const BANDS: [(f32, f32); 4] = [(250.0, 1.0), (1000.0, 1.4), (3500.0, 1.8), (10000.0, 0.7)];

fn speech() -> Vec<f32> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/speech");
    let mut x = Vec::new();
    for i in 0..8 {
        let r = hound::WavReader::open(dir.join(format!("1272-128104-000{i}.wav")))
            .expect("missing testdata: run scripts/fetch-testdata.sh");
        x.extend(r.into_samples::<f32>().map(|s| s.unwrap()));
    }
    x
}

/// Applies a microphone colouration.
fn colour(x: &[f32], filters: &[Coefs]) -> Vec<f32> {
    let mut f: Vec<Biquad> = filters.iter().map(|c| Biquad::new(*c)).collect();
    x.iter()
        .map(|&v| f.iter_mut().fold(v, |acc, b| b.tick(acc)))
        .collect()
}

/// Long-term level of each EQ band relative to the full band, dB.
fn balance(x: &[f32]) -> [f32; 4] {
    let full: f64 = x.iter().map(|v| (*v as f64).powi(2)).sum();
    let mut out = [0.0; 4];
    for (i, (hz, q)) in BANDS.iter().enumerate() {
        let mut f = Biquad::new(Coefs::bandpass(*hz, *q));
        let e: f64 = x.iter().map(|&v| (f.tick(v) as f64).powi(2)).sum();
        out[i] = (10.0 * (e / full).log10()) as f32;
    }
    out
}

fn calibrate(x: &[f32]) -> app_core::VoiceProfile {
    let mut c = Calibrator::new();
    let silence = vec![0.0001f32; 5 * 48_000];
    c.feed(&silence);
    let take = &x[..10 * 48_000];
    assert_eq!(c.feed(take), Phase::Done);
    c.result().unwrap().clone().expect("calibration failed")
}

fn spread(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

#[test]
#[ignore = "needs testdata/speech (run scripts/fetch-testdata.sh)"]
fn processing_reduces_microphone_differences() {
    let voice = speech();
    // A bassy, dark microphone and a thin, bright one.
    let bassy = colour(
        &voice,
        &[
            Coefs::low_shelf(200.0, 6.0),
            Coefs::high_shelf(6000.0, -4.0),
        ],
    );
    let thin = colour(
        &voice,
        &[
            Coefs::low_shelf(300.0, -6.0),
            Coefs::high_shelf(4000.0, 4.0),
        ],
    );
    let before = spread(&balance(&bassy), &balance(&thin));
    let mut after_bal = Vec::new();
    for x in [&bassy, &thin] {
        let profile = calibrate(x);
        let s = ChainSettings {
            modules: settings_for(PresetId::Podcast, 60, None, &profile),
            ..ChainSettings::default()
        };
        let mut chain = Chain::new(PathKind::Main, dsp::BLOCK_MAIN);
        chain.set_settings(&s);
        let mut y = x.clone();
        for b in y.chunks_mut(dsp::BLOCK_MAIN) {
            chain.process(b);
        }
        after_bal.push(balance(&y[5 * 48_000..]));
        eprintln!("eq correction {:?}", profile.eq_correction_db);
    }
    let after = spread(&after_bal[0], &after_bal[1]);
    eprintln!(
        "tonal balance: max band difference between microphones {before:.1} dB before, {after:.1} dB after (target curve {TARGET_CURVE_DB:?})"
    );
    eprintln!(
        "bands after: bassy {:?}, thin {:?}",
        after_bal[0], after_bal[1]
    );
    assert!(
        after < 0.6 * before,
        "before {before:.1} dB, after {after:.1} dB"
    );
}
