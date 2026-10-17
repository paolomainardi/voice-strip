//! Dynamics per preset on real speech: Narration keeps noticeably more
//! dynamic range than Broadcast (EBU loudness range, LRA).

use app_core::{PresetId, VoiceProfile, settings_for};
use dsp::{Chain, ChainSettings, PathKind};

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

fn measure(x: &[f32]) -> (f64, f64) {
    let mut m = ebur128::EbuR128::new(1, 48_000, ebur128::Mode::I | ebur128::Mode::LRA).unwrap();
    m.add_frames_f32(x).unwrap();
    (m.loudness_global().unwrap(), m.loudness_range().unwrap())
}

#[test]
#[ignore = "needs testdata/speech (run scripts/fetch-testdata.sh)"]
fn narration_keeps_more_dynamic_range_than_broadcast() {
    let x = speech();
    let (input_lufs, input_lra) = measure(&x);
    let profile = VoiceProfile {
        trim_db: -23.0 - input_lufs as f32,
        ..VoiceProfile::default()
    };
    let mut lra = Vec::new();
    for preset in [PresetId::Broadcast, PresetId::Narration] {
        let s = ChainSettings {
            modules: settings_for(preset, 60, None, &profile),
            ..ChainSettings::default()
        };
        let mut chain = Chain::new(PathKind::Main, dsp::BLOCK_MAIN);
        chain.set_settings(&s);
        let mut y = x.clone();
        for b in y.chunks_mut(dsp::BLOCK_MAIN) {
            chain.process(b);
        }
        let (l, r) = measure(&y[10 * 48_000..]);
        eprintln!("dynamics: {preset:?}: {l:.1} LUFS, LRA {r:.1} LU");
        lra.push(r);
    }
    eprintln!("dynamics: input LRA {input_lra:.1} LU");
    assert!(
        lra[1] >= lra[0] + 1.0,
        "Narration LRA {:.1} LU vs Broadcast {:.1} LU",
        lra[1],
        lra[0]
    );
}
