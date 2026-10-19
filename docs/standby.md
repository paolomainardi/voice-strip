# Standby

When no application reads the Voice Strip microphone, Voice Strip releases the physical microphone and processes nothing: the microphone-in-use indicator (the orange dot on macOS) goes off, and the app costs almost no CPU or battery. Capture comes back as soon as an application opens Voice Strip.

## When

- **Pause** 5 s (`engine::standby::STANDBY_AFTER`) after the last application stopped reading the virtual microphone, so an application that reopens it at once does not make capture flap.
- **Resume** at once when an application reads it, the headphone monitor is on, or a calibration runs (the controller holds capture through `EngineHandle::set_capture_held`).
- **Unknown counts as needed:** until a backend can tell (the PipeWire graph not listed yet), capture stays on.

The ON button reads STANDBY with its lamp dimmed, the advanced view's Mic pill reads Standby, and `voice-strip ctl status` reports `"capture": "standby"`. ON stays the user's setting: an application opening Voice Strip gets processed audio.

## Where it is decided

The policy (`engine::standby::Policy`) runs on each backend's own non-realtime thread, not in the UI loop: a window on a hidden workspace gets no frames from the compositor, and a policy tied to frames would stop deciding, so a call could start in standby and get silence.

| | Readers | Pause | Thread |
|---|---|---|---|
| Linux | registry links whose output node is `voice_strip_mic` | drop the microphone link, `pw_filter_set_active(false)`; WirePlumber then suspends the source | PipeWire main loop, every 100 ms |
| macOS | `kAudioDevicePropertyDeviceIsRunningSomewhere` on the Voice Strip device, twice a second | stop and destroy the capture IOProc | the backend's control thread |

On macOS the device's own `DeviceIsRunning` only reflects the calling process, so it never sees other applications; `crates/vdev-macos/examples/running.rs` prints both properties.

On resume the main chain drops the frames its neural stage had in flight from before the pause (`Chain::resume_after_gap`); the leveler keeps its gain, so a call starts at the right level.

## Cost

Apple M4 Pro, 0.4.0 (macOS `ps`, recent CPU):

| State | CPU |
|---|---|
| Standby, window shown | 1.5% (28% before) |
| Standby, window hidden | 0.4% |
| Bypass | 0.3% (12% before) |
| Capturing, monitor on, window shown | 18.5% (25.6% before) |
| Capturing, monitor on, window hidden | 6.5% |

Ryzen 9 9950X, Linux, PipeWire: standby 0.2% or less; capturing with light noise removal about 11.5%, of which noise removal 8.4% (about 0.8 ms per 10 ms frame of live speech), the data loop 1.4% and voice detection 0.6%.

The window's share comes from redrawing: the faceplate is now tessellated once and reused, the window no longer requests a repaint every frame, and in standby it redraws 4 times a second once the needles have fallen to rest. While capturing, the meters redraw at 30 frames a second.

## Tests

- `just test-standby` runs `scripts/standby-check.sh` against the running app: standby, an application reading the Voice Strip microphone (`ffmpeg` on macOS, `pw-record` on Linux), capture back, standby again. On macOS run it from Terminal (microphone permission).
- `crates/vdev-linux/tests/standby.rs`, part of `just test-pipewire`: on a private PipeWire server, the backend sees readers come and go, pauses by itself without them (no link, no processing) and resumes with one.
- `crates/engine/src/standby.rs`: the policy's grace period, holds and unknown readers.
