//! The noise removal model dialog: asks before downloading the model, then
//! shows the download's progress or failure. The host draws it over either
//! view while `UiState::model_prompt` is set and gives it every input event
//! meanwhile.

use std::fmt::Write as _;

use app_core::{ModelState, UiState, model};

use crate::input::{Action, Clicker, Cursor, Event, Key};
use crate::painter::{Align, Color, Family, Painter, Point, Rect, Stroke, TextStyle};
use crate::theme;

const CARD_W: f32 = 360.0;
const PAD: f32 = 20.0;
const ROW_H: f32 = 20.0;
const LABEL_W: f32 = 72.0;
const BUTTON_H: f32 = 32.0;
const BUTTON_PAD: f32 = 16.0;
const BUTTON_GAP: f32 = 8.0;
const MIB: f32 = 1024.0 * 1024.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button {
    Cancel,
    /// Download, or Retry after a failure.
    Download,
}

struct Palette {
    scrim: Color,
    card: Color,
    border: Color,
    text: Color,
    soft: Color,
    error: Color,
    primary: Color,
    primary_hover: Color,
    primary_text: Color,
    secondary: Color,
    secondary_border: Color,
    track: Color,
    title: TextStyle,
}

fn palette(dark: bool) -> Palette {
    if dark {
        use theme::advanced as th;
        Palette {
            scrim: Color::rgba(0.0, 0.0, 0.0, 0.6),
            card: th::PANEL,
            border: th::BORDER,
            text: th::TEXT,
            soft: th::MUTED,
            error: th::MUTE_ON,
            primary: th::ACCENT,
            primary_hover: th::ACCENT_HOVER,
            primary_text: th::ACCENT_INK,
            secondary: th::CONTROL,
            secondary_border: th::BORDER,
            track: th::CONTROL,
            title: TextStyle::new(Family::PlexSans, 600, 16.0, th::TEXT),
        }
    } else {
        use theme::standard as th;
        Palette {
            scrim: Color::rgba(0.16, 0.12, 0.09, 0.55),
            card: th::FACEPLATE,
            border: th::FRAME,
            text: th::INK,
            soft: th::INK_SOFT,
            error: th::RED,
            primary: th::AMBER,
            primary_hover: th::LED_LOW,
            primary_text: th::INK,
            secondary: th::BUTTON,
            secondary_border: th::BUTTON_BORDER,
            track: th::BUTTON,
            title: TextStyle::new(Family::Oswald, 600, 15.0, th::INK).spacing(2.0),
        }
    }
}

pub struct ModelDialog {
    clicker: Clicker<Button>,
    /// Buttons where the last paint put them.
    buttons: Vec<(Button, Rect)>,
    /// The cache folder, with the home folder shortened to `~`.
    folder: String,
    text: String,
}

impl Default for ModelDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelDialog {
    pub fn new() -> Self {
        let folder = model::cache_path()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .map(|d| {
                let d = d.to_string_lossy().into_owned();
                match std::env::var("HOME") {
                    Ok(h) if !h.is_empty() && d.starts_with(&h) => format!("~{}", &d[h.len()..]),
                    _ => d,
                }
            })
            .unwrap_or_default();
        Self {
            clicker: Clicker::new(),
            buttons: Vec::new(),
            folder,
            text: String::new(),
        }
    }

    pub fn is_open(state: &UiState) -> bool {
        state.model_prompt.is_some()
    }

    fn hit(&self, p: Point) -> Option<Button> {
        self.buttons
            .iter()
            .find(|(_, r)| r.contains(p))
            .map(|(b, _)| *b)
    }

    /// Takes every event while the dialog is open.
    pub fn handle(&mut self, ev: Event, state: &UiState, out: &mut Vec<Action>) {
        let downloading = matches!(state.model, ModelState::Downloading(_));
        let press = |b: Button, out: &mut Vec<Action>| match b {
            Button::Cancel => out.push(Action::DismissModelPrompt),
            Button::Download => out.push(Action::DownloadModel),
        };
        match ev {
            Event::PointerMove(pt) => self.clicker.moved(self.hit(pt)),
            Event::PointerDown(pt) => self.clicker.down(self.hit(pt)),
            Event::PointerUp(pt) => {
                if let Some(b) = self.clicker.up(self.hit(pt)) {
                    press(b, out);
                }
            }
            Event::PointerLeave => self.clicker.leave(),
            Event::Key(Key::Escape) => press(Button::Cancel, out),
            Event::Key(Key::Enter) if !downloading => press(Button::Download, out),
            _ => {}
        }
    }

    pub fn cursor(&self) -> Cursor {
        match self.clicker.hover {
            Some(_) => Cursor::Pointer,
            None => Cursor::Default,
        }
    }

    /// Paints the dialog centered over a view of `w` x `h`; `dark` picks
    /// the advanced view's palette, otherwise the faceplate's.
    pub fn paint(&mut self, p: &mut dyn Painter, w: f32, h: f32, state: &UiState, dark: bool) {
        let c = palette(dark);
        let body = TextStyle::new(Family::PlexSans, 400, 13.0, c.text);
        let small = TextStyle::new(Family::PlexSans, 400, 12.0, c.soft);
        let label = TextStyle::new(Family::PlexMono, 400, 11.0, c.soft);
        let value = TextStyle::new(Family::PlexSans, 500, 12.0, c.text);
        let inner = CARD_W - 2.0 * PAD;

        let title = if dark {
            "Noise removal needs a model"
        } else {
            "NOISE REMOVAL NEEDS A MODEL"
        };
        let intro = wrap(
            p,
            "Noise removal runs DeepFilterNet3, a small neural network. It is downloaded \
             once and kept on this computer.",
            &body,
            inner,
        );
        let note = wrap(
            p,
            "It runs on this computer only: no audio is ever sent anywhere.",
            &small,
            inner,
        );
        let error = match &state.model {
            ModelState::Failed(e) => wrap(p, e, &small.color(c.error), inner),
            _ => Vec::new(),
        };
        let line = |p: &mut dyn Painter, s: &TextStyle| p.measure_text("Mg", s).height + 3.0;
        let (title_h, body_h, small_h) = (line(p, &c.title), line(p, &body), line(p, &small));
        let rows = 5.0;
        let card_h = PAD
            + title_h
            + 10.0
            + intro.len() as f32 * body_h
            + 12.0
            + rows * ROW_H
            + 12.0
            + note.len() as f32 * small_h
            + if error.is_empty() {
                0.0
            } else {
                8.0 + error.len() as f32 * small_h
            }
            + 18.0
            + BUTTON_H
            + PAD;

        p.fill_rect(Rect::new(0.0, 0.0, w, h), c.scrim);
        let card = Rect::new(
            ((w - CARD_W) / 2.0).round(),
            ((h - card_h) / 2.0).max(PAD).round(),
            CARD_W,
            card_h,
        );
        p.fill_rounded_rect(card, 10.0, c.card);
        p.border_rounded_rect(card, 10.0, if dark { 1.0 } else { 2.0 }, c.border);

        let x = card.x + PAD;
        let mut y = card.y + PAD;
        p.fill_text(title, x, y, &c.title, Align::Left);
        y += title_h + 10.0;
        for l in &intro {
            p.fill_text(l, x, y, &body, Align::Left);
            y += body_h;
        }
        y += 12.0;
        self.text.clear();
        let _ = write!(self.text, "{:.1} MB", model::SIZE as f32 / MIB);
        let size = std::mem::take(&mut self.text);
        let folder = elide_start(p, &self.folder, &value, inner - LABEL_W);
        for (k, v) in [
            ("Size", size.as_str()),
            ("From", model::SOURCE),
            ("Licence", model::LICENCE),
            ("Saved to", folder.as_str()),
            ("Checked", "SHA-256, kept only if it matches"),
        ] {
            p.text_in_rect(k, Rect::new(x, y, LABEL_W, ROW_H), &label, Align::Left);
            p.text_in_rect(
                v,
                Rect::new(x + LABEL_W, y, inner - LABEL_W, ROW_H),
                &value,
                Align::Left,
            );
            y += ROW_H;
        }
        self.text = size;
        y += 12.0;
        for l in &note {
            p.fill_text(l, x, y, &small, Align::Left);
            y += small_h;
        }
        if !error.is_empty() {
            y += 8.0;
            for l in &error {
                p.fill_text(l, x, y, &small.color(c.error), Align::Left);
                y += small_h;
            }
        }
        y += 18.0;

        // Footer: progress while downloading, otherwise the two buttons.
        self.buttons.clear();
        let right = card.right() - PAD;
        let button_style = TextStyle::new(Family::PlexSans, 600, 13.0, c.text);
        let mut bx = right;
        let footer = match state.model {
            ModelState::Downloading(_) => &[(Button::Cancel, "Cancel")][..],
            ModelState::Failed(_) => &[(Button::Download, "Retry"), (Button::Cancel, "Cancel")],
            _ => &[(Button::Download, "Download"), (Button::Cancel, "Cancel")],
        };
        for &(b, text) in footer {
            let bw = p.measure_text(text, &button_style).width + 2.0 * BUTTON_PAD;
            bx -= bw;
            let r = Rect::new(bx, y, bw, BUTTON_H);
            bx -= BUTTON_GAP;
            let hover = self.clicker.hover == Some(b);
            let (bg, border, fg) = match b {
                Button::Download => (
                    if hover { c.primary_hover } else { c.primary },
                    if dark { c.primary } else { c.text },
                    c.primary_text,
                ),
                Button::Cancel => (
                    c.secondary,
                    if hover { c.text } else { c.secondary_border },
                    c.text,
                ),
            };
            p.fill_rounded_rect(r, 6.0, bg);
            p.border_rounded_rect(r, 6.0, 1.0, border);
            p.text_in_rect(text, r, &button_style.color(fg), Align::Center);
            self.buttons.push((b, r));
        }
        if let ModelState::Downloading(bytes) = state.model {
            let w = bx - x - 8.0;
            self.text.clear();
            let _ = write!(
                self.text,
                "Downloading… {:.1} / {:.1} MB",
                bytes as f32 / MIB,
                model::SIZE as f32 / MIB
            );
            p.fill_text(&self.text, x, y, &label, Align::Left);
            let track = Rect::new(x, y + BUTTON_H - 10.0, w, 6.0);
            p.fill_rounded_rect(track, 3.0, c.track);
            let f = (bytes as f32 / model::SIZE as f32).clamp(0.0, 1.0);
            if f > 0.0 {
                p.fill_rounded_rect(
                    Rect::new(track.x, track.y, (track.w * f).max(track.h), track.h),
                    3.0,
                    c.primary,
                );
            }
        }
    }
}

/// Small "download needed" mark, 7 x 9 with its top-left at `(x, y)`: an
/// arrow down onto a line.
pub fn download_mark(p: &mut dyn Painter, x: f32, y: f32, color: Color) {
    let s = Stroke::new(color, 1.2);
    let cx = x + 3.5;
    p.line(cx, y, cx, y + 6.0, s);
    p.begin_path();
    p.move_to(x + 0.5, y + 3.5);
    p.line_to(cx, y + 6.5);
    p.line_to(x + 6.5, y + 3.5);
    p.stroke(s);
    p.line(x, y + 8.5, x + 7.0, y + 8.5, s);
}

/// Splits `text` into lines no wider than `max` in `style`, at spaces.
fn wrap(p: &mut dyn Painter, text: &str, style: &TextStyle, max: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let len = cur.len();
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
        if len > 0 && p.measure_text(&cur, style).width > max {
            cur.truncate(len);
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// `text` with its start cut to fit `max` (a path keeps its last folders).
fn elide_start(p: &mut dyn Painter, text: &str, style: &TextStyle, max: f32) -> String {
    if p.measure_text(text, style).width <= max {
        return text.to_string();
    }
    let mut start = 0;
    while let Some((i, _)) = text[start..].char_indices().nth(1) {
        start += i;
        let s = format!("…{}", &text[start..]);
        if p.measure_text(&s, style).width <= max {
            return s;
        }
    }
    "…".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::ModelUse;

    fn prompt(model: ModelState) -> UiState {
        let mut s = crate::demo::demo_state();
        s.model = model;
        s.model_prompt = Some(ModelUse::Noise(app_core::DenoiseLevel::Medium));
        s
    }

    #[test]
    fn keys_answer_the_dialog() {
        let mut d = ModelDialog::new();
        let mut out = Vec::new();
        d.handle(
            Event::Key(Key::Enter),
            &prompt(ModelState::Missing),
            &mut out,
        );
        d.handle(
            Event::Key(Key::Escape),
            &prompt(ModelState::Missing),
            &mut out,
        );
        assert_eq!(out, [Action::DownloadModel, Action::DismissModelPrompt]);
        out.clear();
        d.handle(
            Event::Key(Key::Enter),
            &prompt(ModelState::Downloading(10)),
            &mut out,
        );
        assert!(out.is_empty());
    }
}
