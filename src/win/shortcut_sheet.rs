use std::sync::atomic::{AtomicBool, Ordering};

use super::controller_tips::TipToken;
use TipToken::{Button, Plus, Text};

pub const FADE_SECS: f32 = 0.15;

static SHOWN: AtomicBool = AtomicBool::new(false);

pub fn set_shown(on: bool) {
    if SHOWN.swap(on, Ordering::SeqCst) != on {
        super::vk_ui::request_repaint();
    }
}

pub fn shown() -> bool {
    SHOWN.load(Ordering::SeqCst)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SheetRow {
    pub keys: &'static [TipToken],
    pub label: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SheetGroup {
    pub title: &'static str,
    pub rows: &'static [SheetRow],
}

const fn row(keys: &'static [TipToken], label: &'static str) -> SheetRow {
    SheetRow { keys, label }
}

const SLASH: TipToken = Text("/");

pub static GROUPS: [SheetGroup; 3] = [
    SheetGroup {
        title: "Keyboard",
        rows: &[
            row(&[Button("L3")], "Close keyboard"),
            row(&[Button("R3")], "Dictate"),
            row(&[Text("Tap"), Button("SELECT")], "Suggestions"),
            row(&[Button("SELECT"), Plus, Button("X")], "Copy"),
            row(&[Button("SELECT"), Plus, Button("Y")], "Paste"),
            row(&[Button("SELECT"), Plus, Button("B")], "Clear field"),
            row(&[Button("LB"), SLASH, Button("RB")], "Move caret"),
            row(
                &[
                    Text("Hold"),
                    Button("SELECT"),
                    Plus,
                    Button("LB"),
                    SLASH,
                    Button("RB"),
                ],
                "Jump a word",
            ),
            row(&[Button("RT")], "Shift"),
            row(&[Button("LT")], "Symbols"),
            row(&[Button("X")], "Switch layout"),
        ],
    },
    SheetGroup {
        title: "Keyboard closed",
        rows: &[
            row(
                &[Button("LB"), Plus, Button("RB"), Plus, Button("RT")],
                "Screenshot",
            ),
            row(
                &[Button("LB"), Plus, Button("RB"), Plus, Button("LT")],
                "Window screenshot",
            ),
            row(
                &[Button("SELECT"), Plus, Button("LB"), Plus, Button("X")],
                "Open warmUP",
            ),
        ],
    },
    SheetGroup {
        title: "In games",
        rows: &[
            row(
                &[Text("Hold"), Button("SELECT"), Plus, Button("LB")],
                "Screenshot",
            ),
            row(
                &[Text("Hold"), Button("SELECT"), Plus, Button("RB")],
                "Record (warmUP)",
            ),
        ],
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Line {
    Title(&'static str),
    Blank,
    Row(SheetRow),
}

pub fn columns() -> [Vec<Line>; 3] {
    let keyboard = &GROUPS[0];
    let split = keyboard.rows.len().div_ceil(2);
    let mut first = vec![Line::Title(keyboard.title)];
    first.extend(keyboard.rows[..split].iter().copied().map(Line::Row));
    let mut second = vec![Line::Blank];
    second.extend(keyboard.rows[split..].iter().copied().map(Line::Row));
    let mut third = Vec::new();
    for group in &GROUPS[1..] {
        third.push(Line::Title(group.title));
        third.extend(group.rows.iter().copied().map(Line::Row));
    }
    [first, second, third]
}

pub fn max_lines() -> usize {
    columns().iter().map(Vec::len).max().unwrap_or(1).max(1)
}

pub fn fade_step(alpha: f32, on: bool, dt_secs: f32) -> f32 {
    let step = dt_secs.max(0.0) / FADE_SECS;
    if on {
        (alpha + step).min(1.0)
    } else {
        (alpha - step).max(0.0)
    }
}

#[cfg(test)]
pub fn buttons() -> impl Iterator<Item = &'static str> {
    GROUPS
        .iter()
        .flat_map(|g| g.rows.iter())
        .flat_map(|r| r.keys.iter())
        .filter_map(|t| match t {
            Button(b) => Some(*b),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_hold_every_row_once() {
        let total: usize = GROUPS.iter().map(|g| g.rows.len()).sum();
        let rows = columns()
            .iter()
            .flatten()
            .filter(|l| matches!(l, Line::Row(_)))
            .count();
        assert_eq!(rows, total);
        let cols = columns();
        assert_eq!(cols[0][0], Line::Title("Keyboard"));
        assert_eq!(cols[1][0], Line::Blank);
        assert_eq!(cols[2][0], Line::Title("Keyboard closed"));
        assert!(cols
            .iter()
            .all(|c| !matches!(c.last(), Some(Line::Title(_)))));
        assert_eq!(max_lines(), 7);
    }

    #[test]
    fn fade_takes_about_150ms_each_way() {
        let mut a = 0.0;
        for _ in 0..9 {
            a = fade_step(a, true, 0.016);
        }
        assert!(a < 1.0);
        a = fade_step(a, true, 0.016);
        assert_eq!(a, 1.0);
        assert_eq!(fade_step(1.0, false, FADE_SECS), 0.0);
        assert_eq!(fade_step(0.0, false, 0.5), 0.0);
    }

    #[test]
    fn sheet_rows_match_the_mapped_shortcuts() {
        let labels: Vec<&str> = GROUPS
            .iter()
            .flat_map(|g| g.rows.iter())
            .map(|r| r.label)
            .collect();
        for l in [
            "Close keyboard",
            "Dictate",
            "Suggestions",
            "Copy",
            "Paste",
            "Clear field",
            "Move caret",
            "Jump a word",
            "Shift",
            "Screenshot",
            "Window screenshot",
            "Open warmUP",
            "Record (warmUP)",
        ] {
            assert!(labels.contains(&l), "{l}");
        }
        let paste = GROUPS[0].rows.iter().find(|r| r.label == "Paste").unwrap();
        assert_eq!(paste.keys, &[Button("SELECT"), Plus, Button("Y")]);
    }
}
