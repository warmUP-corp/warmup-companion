use std::sync::atomic::{AtomicBool, Ordering};

use super::controller_tips::TipToken;
use TipToken::{Button, Plus, Text};

static SHOWN: AtomicBool = AtomicBool::new(false);

pub const HINT_RETIRE_OPENS: u32 = 3;

pub fn set_shown(on: bool) {
    if SHOWN.swap(on, Ordering::SeqCst) != on {
        super::vk_ui::request_repaint();
        if on {
            let _ = std::thread::Builder::new()
                .name("sheet-opens".into())
                .spawn(record_open);
        }
    }
}

fn record_open() {
    #[cfg(all(feature = "gamepad", not(test)))]
    {
        let opens = crate::config::gamepad_settings().vk_sheet_opens;
        if opens < HINT_RETIRE_OPENS {
            let _ = crate::config::set_gamepad_setting("vk_sheet_opens", &(opens + 1).to_string());
        }
    }
}

pub fn hint_retired() -> bool {
    #[cfg(feature = "gamepad")]
    {
        crate::config::gamepad_settings().vk_sheet_opens >= HINT_RETIRE_OPENS
    }
    #[cfg(not(feature = "gamepad"))]
    {
        false
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

pub static SHEET_HINT: SheetRow = row(&[Text("Hold"), Button("SELECT")], "All shortcuts");

pub static GROUPS: [SheetGroup; 6] = [
    SheetGroup {
        title: "Typing",
        rows: &[
            row(&[Button("A")], "Type"),
            row(&[Button("B")], "Delete"),
            row(&[Button("Y")], "Space"),
            row(&[Button("START")], "Enter"),
            row(&[Button("RT")], "Shift"),
            row(&[Button("LT")], "Symbols"),
        ],
    },
    SheetGroup {
        title: "Editing",
        rows: &[
            row(&[Button("SELECT"), Plus, Button("X")], "Copy"),
            row(&[Button("SELECT"), Plus, Button("Y")], "Paste"),
            row(&[Button("SELECT"), Plus, Button("B")], "Clear field"),
        ],
    },
    SheetGroup {
        title: "Cursor",
        rows: &[
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
        ],
    },
    SheetGroup {
        title: "Keyboard",
        rows: &[
            row(&[Text("Tap"), Button("SELECT")], "Suggestions"),
            row(&[Button("R3")], "Dictate"),
            row(&[Button("X")], "Switch layout"),
            row(&[Button("L3")], "Close keyboard"),
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

pub fn shown_rows(group: &SheetGroup) -> Vec<SheetRow> {
    group
        .rows
        .iter()
        .copied()
        .filter(|row| row.label != "Dictate" || show_voice_shortcut())
        .collect()
}

fn show_voice_shortcut() -> bool {
    crate::config::voice_enabled()
}

#[cfg(test)]
pub fn buttons() -> impl Iterator<Item = &'static str> {
    GROUPS
        .iter()
        .flat_map(|g| g.rows.iter())
        .chain(std::iter::once(&SHEET_HINT))
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
    fn groups_are_sorted_into_six_tiles() {
        let titles: Vec<&str> = GROUPS.iter().map(|g| g.title).collect();
        assert_eq!(
            titles,
            [
                "Typing",
                "Editing",
                "Cursor",
                "Keyboard",
                "Keyboard closed",
                "In games"
            ]
        );
        assert!(GROUPS.iter().all(|g| !g.rows.is_empty()));
    }

    #[test]
    fn hint_points_at_the_sheet() {
        assert_eq!(SHEET_HINT.label, "All shortcuts");
        assert_eq!(SHEET_HINT.keys, &[Text("Hold"), Button("SELECT")]);
    }

    #[test]
    fn sheet_rows_match_the_mapped_shortcuts() {
        let labels: Vec<&str> = GROUPS
            .iter()
            .flat_map(|g| g.rows.iter())
            .map(|r| r.label)
            .collect();
        for l in [
            "Type",
            "Delete",
            "Space",
            "Enter",
            "Close keyboard",
            "Dictate",
            "Suggestions",
            "Copy",
            "Paste",
            "Clear field",
            "Move caret",
            "Jump a word",
            "Shift",
            "Symbols",
            "Switch layout",
            "Screenshot",
            "Window screenshot",
            "Open warmUP",
            "Record (warmUP)",
        ] {
            assert!(labels.contains(&l), "{l}");
        }
        let paste = GROUPS[1].rows.iter().find(|r| r.label == "Paste").unwrap();
        assert_eq!(paste.keys, &[Button("SELECT"), Plus, Button("Y")]);
    }
}
