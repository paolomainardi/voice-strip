//! Toolkit-neutral widgets and views, drawn through a [`Painter`] trait.
//! Frontends (GTK on Linux, Core Graphics on macOS) implement `Painter`,
//! forward input as [`Event`]s and apply the returned [`Action`]s to
//! `app_core::Controller`.

pub mod advanced;
pub mod ballistics;
pub mod demo;
pub mod help;
pub mod input;
pub mod knob;
pub mod model_dialog;
pub mod painter;
pub mod remote;
pub mod spectrum;
pub mod standard;
pub mod theme;
pub mod view;
pub mod vu;

pub use advanced::AdvancedView;
pub use input::{Action, Cursor, Event, Key, ViewKind};
pub use model_dialog::ModelDialog;
pub use painter::{
    Align, Color, Family, LineCap, Painter, Point, Rect, Stroke, TextSize, TextStyle,
};
pub use standard::StandardView;
pub use view::View;
