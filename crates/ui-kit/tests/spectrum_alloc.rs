//! The analyzer runs every frame on the UI thread: no allocation once it
//! is built.

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use ui_kit::spectrum::Analyzer;

#[global_allocator]
static A: AllocDisabler = AllocDisabler;

#[test]
fn analysis_and_ballistics_do_not_allocate() {
    let mut a = Analyzer::new(48_000.0);
    let frame: Vec<f32> = (0..800).map(|i| (i as f32 * 0.07).sin() * 0.3).collect();
    // Warm up, then check a few seconds of frames, including the hops that
    // land several per frame.
    a.push(&frame);
    a.update(1.0 / 60.0);
    assert_no_alloc(|| {
        for _ in 0..180 {
            a.push(&frame);
            a.update(1.0 / 60.0);
        }
        a.push(&[0.0; 4096]);
        a.update(0.1);
    });
    assert!(a.analyses().1 > 100);
}
