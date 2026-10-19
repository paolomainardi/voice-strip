//! Tray commands and state, shared with the macOS menu bar
//! ([`crate::menubar`]), and the Linux system tray icon (StatusNotifierItem,
//! shown by KDE, GNOME with the AppIndicator extension, waybar and most
//! Linux panels). A thin native layer: it mirrors the controller state and
//! sends commands back to the UI thread through a channel.

#[cfg(target_os = "linux")]
use std::sync::mpsc::Sender;

#[cfg(target_os = "linux")]
use app_core::DenoiseLevel;
use app_core::PresetId;

/// What the user picked in the tray menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrayCmd {
    SetOn(bool),
    SetMuted(bool),
    SetPreset(PresetId),
    SetNoise(Option<app_core::DenoiseLevel>),
    Show,
    /// The window item: hides the window when it is shown, shows it
    /// otherwise.
    ToggleWindow,
    Quit,
}

/// State mirrored from the controller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrayState {
    pub on: bool,
    pub muted: bool,
    pub preset: PresetId,
    pub noise: Option<app_core::DenoiseLevel>,
    /// Whether the window is shown.
    pub window: bool,
}

impl TrayState {
    pub fn of(st: &app_core::UiState, window: bool) -> Self {
        Self {
            on: st.on,
            muted: st.muted,
            preset: st.preset,
            noise: st.neural_denoise,
            window,
        }
    }

    /// Label of the window item.
    pub fn window_label(&self) -> &'static str {
        if self.window {
            "Hide window"
        } else {
            "Show window"
        }
    }
}

#[cfg(target_os = "linux")]
pub struct VoiceStripTray {
    tx: Sender<TrayCmd>,
    pub state: TrayState,
}

#[cfg(target_os = "linux")]
impl VoiceStripTray {
    pub fn new(tx: Sender<TrayCmd>, state: TrayState) -> Self {
        Self { tx, state }
    }
}

#[cfg(target_os = "linux")]
fn preset_label(p: PresetId) -> &'static str {
    app_core::presets::builtin(p).label
}

#[cfg(target_os = "linux")]
impl ksni::Tray for VoiceStripTray {
    fn id(&self) -> String {
        "voice-strip".into()
    }

    fn title(&self) -> String {
        "Voice Strip".into()
    }

    fn icon_name(&self) -> String {
        if self.state.muted {
            "microphone-sensitivity-muted".into()
        } else if self.state.on {
            "audio-input-microphone".into()
        } else {
            "microphone-sensitivity-muted".into()
        }
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Voice Strip".into(),
            description: format!(
                "{} · {}",
                if self.state.on {
                    preset_label(self.state.preset)
                } else {
                    "Bypass"
                },
                match self.state.noise {
                    Some(l) => format!("noise removal {}", l.label().to_lowercase()),
                    None => "noise removal off".into(),
                }
            ),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(TrayCmd::Show);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        let presets = PresetId::ALL;
        let selected_preset = presets
            .iter()
            .position(|p| *p == self.state.preset)
            .unwrap_or(0);
        let noise_selected = match self.state.noise {
            None => 0,
            Some(l) => DenoiseLevel::ALL
                .iter()
                .position(|x| *x == l)
                .map_or(0, |i| i + 1),
        };
        let mut noise_options = vec![RadioItem {
            label: "Off".into(),
            ..Default::default()
        }];
        noise_options.extend(DenoiseLevel::ALL.iter().map(|l| RadioItem {
            label: l.label().into(),
            ..Default::default()
        }));
        vec![
            CheckmarkItem {
                label: "Processing on".into(),
                checked: self.state.on,
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send(TrayCmd::SetOn(!t.state.on));
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Mute".into(),
                checked: self.state.muted,
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send(TrayCmd::SetMuted(!t.state.muted));
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            SubMenu {
                label: "Preset".into(),
                submenu: vec![
                    RadioGroup {
                        selected: selected_preset,
                        select: Box::new(|t: &mut Self, i| {
                            let _ = t.tx.send(TrayCmd::SetPreset(PresetId::ALL[i]));
                        }),
                        options: presets
                            .iter()
                            .map(|p| RadioItem {
                                label: preset_label(*p).into(),
                                ..Default::default()
                            })
                            .collect(),
                    }
                    .into(),
                ],
                ..Default::default()
            }
            .into(),
            SubMenu {
                label: "Noise removal".into(),
                submenu: vec![
                    RadioGroup {
                        selected: noise_selected,
                        select: Box::new(|t: &mut Self, i| {
                            let level = if i == 0 {
                                None
                            } else {
                                Some(DenoiseLevel::ALL[i - 1])
                            };
                            let _ = t.tx.send(TrayCmd::SetNoise(level));
                        }),
                        options: noise_options,
                    }
                    .into(),
                ],
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: self.state.window_label().into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send(TrayCmd::ToggleWindow);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send(TrayCmd::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}
