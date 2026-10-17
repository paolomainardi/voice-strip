//! Recalibrate from the controller: 15 s of input through the engine's
//! scope queue produce a voice profile that is applied to the chain.

use app_core::Controller;
use app_core::calibration::Phase;

#[test]
fn calibration_runs_through_the_controller() {
    let (handle, mut processor) = engine::new_engine();
    processor.set_host_block_size(Some(480));
    let mut c = Controller::new(handle);
    c.start_calibration();
    assert_eq!(
        c.state().calibration.as_ref().unwrap().phase,
        Phase::Silence
    );
    let mut out = vec![0.0f32; 480];
    let mut rng = 1u64;
    let mut noise = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 40) as f32 / 8_388_608.0 - 1.0
    };
    for k in 0..1_600 {
        let t0 = k * 480;
        let block: Vec<f32> = (0..480)
            .map(|i| {
                let t = (t0 + i) as f32 / 48_000.0;
                let voice = if t >= 5.0 {
                    0.2 * (std::f32::consts::TAU * 220.0 * t).sin()
                        * (0.5 - 0.5 * (std::f32::consts::TAU * 4.0 * t).cos())
                } else {
                    0.0
                };
                voice + 0.0005 * noise()
            })
            .collect();
        processor.process(&block, &mut out, None);
        c.tick();
        if !c.is_calibrating() {
            break;
        }
    }
    let st = c.state().calibration.clone().unwrap();
    eprintln!("calibration: {:?}", st);
    assert_eq!(st.phase, Phase::Done);
    assert!(!st.error, "{:?}", st.message);
    assert_eq!(st.label(), "Calibrated");
    assert_eq!(c.state().voice_profile, "Calibrated");
}
