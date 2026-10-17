//! Runs the physical microphone through a neutral chain into the virtual
//! microphone `voice_strip_mic` until Ctrl-C.
//!
//! `cargo run -p vdev-linux --example passthrough -- [input-node-name] [quantum]`

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("passthrough: PipeWire runs on Linux only");
}

#[cfg(target_os = "linux")]
fn main() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use engine::RunningBackend;
    use vdev_linux::PipeWireBackend;

    let mut args = std::env::args().skip(1);
    let input = args.next();
    let quantum: u32 = args.next().and_then(|q| q.parse().ok()).unwrap_or(480);

    let (handle, processor) = engine::new_engine();
    let backend = PipeWireBackend::new().with_quantum(quantum);
    let running = match backend.start_pipewire(processor, input.as_deref(), None) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("start failed: {e}");
            std::process::exit(1);
        }
    };
    println!("virtual microphone: {}", running.virtual_device_name());
    for d in vdev_linux::list_devices(true).unwrap_or_default() {
        println!("  input: {} ({})", d.id, d.label);
    }

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || stop.store(true, Ordering::Relaxed)).expect("signal handler");
    }
    let mut last = (false, 0);
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
        if let Some(e) = running.error() {
            eprintln!("backend error: {e}");
            break;
        }
        let now = (running.is_linked(), running.quantum());
        if now != last {
            last = now;
            let (io, _) = running.io_latency_samples();
            println!(
                "linked={} quantum={} engine latency={} samples, driver delay={} samples",
                now.0,
                now.1,
                handle.main_latency_samples(),
                io
            );
        }
    }
    drop(running);
    println!("stopped");
}
