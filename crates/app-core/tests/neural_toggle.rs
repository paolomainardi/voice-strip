//! Turning neural noise removal on adds its 20 ms to the mic path; turning
//! it off removes them again.

use app_core::{Controller, DenoiseLevel, PresetId};

#[test]
fn neural_toggle_changes_mic_latency() {
    let (handle, mut processor) = engine::new_engine();
    processor.set_host_block_size(Some(480));
    let mut c = Controller::new(handle);
    let input = vec![0.0f32; 480];
    let mut out = vec![0.0f32; 480];
    let mut step = |c: &mut Controller| {
        processor.process(&input, &mut out, None);
        c.tick();
        c.state().mic_latency_ms
    };
    c.set_preset(PresetId::Broadcast);
    let off = step(&mut c);
    c.set_neural_denoise(Some(DenoiseLevel::Medium));
    // The model loads in the background; the chain swaps when it is ready.
    let mut on = step(&mut c);
    for _ in 0..2_000 {
        if on > off + 10.0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        on = step(&mut c);
    }
    c.set_neural_denoise(None);
    let off_again = step(&mut c);
    eprintln!("mic latency: off {off:.2} ms, on {on:.2} ms, off {off_again:.2} ms");
    assert!((on - off - 20.0).abs() < 0.01, "on {on} off {off}");
    assert!((off_again - off).abs() < 0.01);
    // Call defaults to strong noise removal.
    c.set_preset(PresetId::Call);
    assert_eq!(c.state().neural_denoise, Some(DenoiseLevel::Strong));
    let mut call = step(&mut c);
    for _ in 0..2_000 {
        if (call - on).abs() < 0.01 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        call = step(&mut c);
    }
    assert!((call - on).abs() < 0.01, "call {call} on {on}");
}

/// Turning on "denoise in headphones" adds the neural stage to the monitor
/// path (20 ms at a 480 host block); turning it off removes it.
#[test]
fn monitor_denoise_changes_monitor_latency() {
    let (handle, mut processor) = engine::new_engine();
    processor.set_host_block_size(Some(480));
    let mut c = Controller::new(handle);
    let input = vec![0.0f32; 480];
    let mut out = vec![0.0f32; 480];
    let mut mon = vec![0.0f32; 480];
    let mut step = |c: &mut Controller| {
        processor.process(&input, &mut out, Some(&mut mon));
        c.tick();
        c.state().monitor_latency_ms
    };
    let mut cfg = engine::MonitorConfig {
        enabled: true,
        with_denoise: false,
        level: 1.0,
    };
    c.set_monitor(cfg);
    let plain = step(&mut c);
    cfg.with_denoise = true;
    c.set_monitor(cfg);
    let mut with = step(&mut c);
    for _ in 0..2_000 {
        if with > plain + 10.0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        with = step(&mut c);
    }
    cfg.with_denoise = false;
    c.set_monitor(cfg);
    let back = step(&mut c);
    eprintln!("monitor latency: plain {plain:.2} ms, with denoise {with:.2} ms, back {back:.2} ms");
    assert!(
        (with - plain - 20.0).abs() < 0.01,
        "with {with} plain {plain}"
    );
    assert!((back - plain).abs() < 0.01);
}
