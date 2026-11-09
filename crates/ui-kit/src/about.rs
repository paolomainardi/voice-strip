//! Standard faceplate's About popup, drawn and hit-tested in view coordinates.

use app_core::UiState;

use crate::input::{Action, Clicker, Cursor, Event, Key};
use crate::painter::{Align, Family, Painter, Point, Rect, Stroke, TextStyle};
use crate::theme::standard as th;

const CARD: Rect = Rect::new(22.0, 82.0, 280.0, 200.0);
const UPDATE: Rect = Rect::new(40.0, 128.0, 244.0, 22.0);
const RELEASES: Rect = Rect::new(40.0, 251.0, 82.0, 24.0);
const PROBLEM: Rect = Rect::new(130.0, 251.0, 120.0, 24.0);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Link {
    Update,
    Releases,
    Problem,
}

pub struct AboutPopup {
    pub open: bool,
    clicker: Clicker<Link>,
    /// Keep the release of an outside press away from the view underneath.
    dismissing: bool,
}

impl Default for AboutPopup {
    fn default() -> Self {
        Self {
            open: false,
            clicker: Clicker::new(),
            dismissing: false,
        }
    }
}

pub fn update_text(state: &UiState) -> String {
    if !state.update_check {
        "Update check off".into()
    } else if let Some(version) = &state.update_available {
        format!("{version} available: what's new")
    } else if state.update_checked {
        "Up to date".into()
    } else {
        "Update check pending".into()
    }
}

impl AboutPopup {
    fn hit(&self, pt: Point, state: &UiState) -> Option<Link> {
        if state.update_check && state.update_available.is_some() && UPDATE.contains(pt) {
            Some(Link::Update)
        } else if RELEASES.contains(pt) {
            Some(Link::Releases)
        } else if PROBLEM.contains(pt) {
            Some(Link::Problem)
        } else {
            None
        }
    }

    /// Returns whether the popup consumed the event.
    pub fn handle(&mut self, ev: Event, state: &UiState, out: &mut Vec<Action>) -> bool {
        if self.dismissing {
            if matches!(ev, Event::PointerUp(_) | Event::PointerLeave) {
                self.dismissing = false;
            }
            return true;
        }
        if !self.open {
            return false;
        }
        match ev {
            Event::PointerDown(pt) if !CARD.contains(pt) => {
                self.open = false;
                self.dismissing = true;
                self.clicker.leave();
            }
            Event::PointerDown(pt) => self.clicker.down(self.hit(pt, state)),
            Event::PointerUp(pt) => {
                if let Some(link) = self.clicker.up(self.hit(pt, state)) {
                    out.push(match link {
                        Link::Update => Action::OpenAboutUpdate,
                        Link::Releases => Action::OpenReleaseNotes,
                        Link::Problem => Action::OpenProblem,
                    });
                }
            }
            Event::PointerMove(pt) => self.clicker.moved(self.hit(pt, state)),
            Event::PointerLeave => self.clicker.leave(),
            Event::Key(Key::Escape) => {
                self.open = false;
                self.clicker.leave();
            }
            _ => {}
        }
        true
    }

    pub fn cursor(&self) -> Cursor {
        if self.clicker.hover.is_some() {
            Cursor::Pointer
        } else {
            Cursor::Default
        }
    }

    pub fn paint(&self, p: &mut dyn Painter, state: &UiState) {
        if !self.open {
            return;
        }
        p.fill_rounded_rect(CARD, 8.0, th::VU_FACE);
        p.border_rounded_rect(CARD, 8.0, 2.0, th::INK);
        let title = TextStyle::new(Family::Oswald, 600, 15.0, th::INK).spacing(2.0);
        let body = TextStyle::new(Family::PlexMono, 400, 11.0, th::INK_SOFT);
        let label = TextStyle::new(Family::Oswald, 400, 12.0, th::INK).spacing(1.0);
        p.fill_text("VOICE STRIP", 40.0, 98.0, &title, Align::Left);
        p.fill_text(
            &state.app_version,
            284.0,
            103.0,
            &body.color(th::INK),
            Align::Right,
        );
        let lamp =
            if !state.update_check || !state.update_checked && state.update_available.is_none() {
                th::LAMP_OFF
            } else if state.update_available.is_some() {
                th::AMBER
            } else {
                th::GREEN
            };
        p.fill_circle(44.0, 138.0, 4.0, lamp);
        p.text_in_rect(
            &update_text(state),
            Rect::new(56.0, UPDATE.y, 228.0, UPDATE.h),
            &body.color(if state.update_available.is_some() {
                th::LINK_HOVER
            } else {
                th::INK_SOFT
            }),
            Align::Left,
        );
        for y in [155.0, 243.0] {
            p.line(40.0, y, 284.0, y, Stroke::new(th::RULE, 1.0));
        }
        for (name, value, y) in [
            ("MICROPHONE", state.input_device.clone(), 163.0),
            ("OUTPUT", state.virtual_device.clone(), 191.0),
            ("LATENCY", format!("{:.1} ms", state.mic_latency_ms), 219.0),
        ] {
            p.text_in_rect(name, Rect::new(40.0, y, 82.0, 20.0), &label, Align::Left);
            let r = Rect::new(122.0, y, 162.0, 20.0);
            let value = elide(p, &value, &body, r.w);
            p.text_in_rect(&value, r, &body, Align::Left);
        }
        for (text, rect) in [("RELEASE NOTES", RELEASES), ("REPORT A PROBLEM", PROBLEM)] {
            p.text_in_rect(text, rect, &label.color(th::LINK_HOVER), Align::Left);
        }
    }
}

fn elide(p: &mut dyn Painter, text: &str, style: &TextStyle, max: f32) -> String {
    let mut value = text.to_string();
    if p.measure_text(&value, style).width > max {
        while !value.is_empty() && p.measure_text(&format!("{value}…"), style).width > max {
            value.pop();
        }
        value.push('…');
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_links_require_matching_press_and_release() {
        let mut popup = AboutPopup {
            open: true,
            ..Default::default()
        };
        let mut s = crate::demo::demo_state();
        let mut out = Vec::new();
        for (r, action) in [
            (RELEASES, Action::OpenReleaseNotes),
            (PROBLEM, Action::OpenProblem),
        ] {
            popup.handle(Event::PointerDown(r.center()), &s, &mut out);
            popup.handle(Event::PointerUp(r.center()), &s, &mut out);
            assert_eq!(out.pop(), Some(action));
        }
        popup.handle(Event::PointerDown(RELEASES.center()), &s, &mut out);
        popup.handle(Event::PointerUp(PROBLEM.center()), &s, &mut out);
        assert!(out.is_empty());
        popup.handle(Event::PointerDown(UPDATE.center()), &s, &mut out);
        popup.handle(Event::PointerUp(UPDATE.center()), &s, &mut out);
        assert!(out.is_empty());
        s.update_available = Some("0.7.6".into());
        popup.handle(Event::PointerDown(UPDATE.center()), &s, &mut out);
        popup.handle(Event::PointerUp(UPDATE.center()), &s, &mut out);
        assert_eq!(out, [Action::OpenAboutUpdate]);
        assert_eq!(popup.hit(Point::new(CARD.right(), CARD.bottom()), &s), None);
    }

    #[test]
    fn update_copy_matches_check_state() {
        let mut s = crate::demo::demo_state();
        assert_eq!(update_text(&s), "Up to date");
        s.update_checked = false;
        assert_eq!(update_text(&s), "Update check pending");
        s.update_available = Some("0.7.6".into());
        assert_eq!(update_text(&s), "0.7.6 available: what's new");
        s.update_check = false;
        assert_eq!(update_text(&s), "Update check off");
    }
}
