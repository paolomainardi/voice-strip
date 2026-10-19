//! Remote control (`voice-strip ctl`, desktop bars): the toolkit-neutral
//! server from `app_core::remote`, served once per frame like the tray.

use app_core::Controller;
use app_core::remote::{self, DisplayMeters, Effect, Extras, server::Server};
use ui_kit::remote::RemoteMeters;

/// Watch lines go out at most this often, seconds.
const PUBLISH_PERIOD: f32 = 1.0 / 30.0;

pub struct Remote {
    server: Server,
    ballistics: RemoteMeters,
    meters: DisplayMeters,
    fonts: String,
    since_publish: f32,
}

impl Remote {
    /// Listens on the control socket; `None` when another Voice Strip
    /// already listens or there is no runtime folder, with a message unless
    /// `quiet` (retries).
    pub fn start(quiet: bool) -> Option<Self> {
        match Server::start() {
            Ok(server) => Some(Self {
                server,
                ballistics: RemoteMeters::new(),
                meters: DisplayMeters::default(),
                fonts: crate::fonts::files_dir().to_string_lossy().into_owned(),
                since_publish: 0.0,
            }),
            Err(e) => {
                if !quiet {
                    eprintln!("voice-strip: no remote control ({e}); retrying");
                }
                None
            }
        }
    }

    /// Answers pending commands and, while a client watches, publishes the
    /// state with live meters. Returns what the window still has to do.
    pub fn frame(&mut self, controller: &mut Controller, dt: f32) -> Effect {
        let Self {
            server,
            ballistics,
            meters,
            fonts,
            since_publish,
        } = self;
        let mut effect = Effect::None;
        server.poll(|command| {
            match remote::apply(controller, command) {
                Ok(Effect::None) => {}
                // A quit asked for in the same batch is never undone.
                Ok(e) if effect != Effect::Quit => effect = e,
                Ok(_) => {}
                Err(e) => return remote::error_json(&e),
            }
            remote::state_json(
                controller.state(),
                Extras {
                    meters: Some(meters),
                    presets: true,
                    fonts_dir: Some(fonts),
                },
            )
        });
        if server.is_watched() {
            *meters = ballistics.update(controller.state(), dt);
            *since_publish += dt;
            // A little slack, so a loop that runs about every period does
            // not skip every other one.
            if *since_publish >= PUBLISH_PERIOD * 0.8 {
                *since_publish = 0.0;
                server.publish(remote::state_json(
                    controller.state(),
                    Extras {
                        meters: Some(meters),
                        ..Default::default()
                    },
                ));
            }
        }
        effect
    }
}
