use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "gamepad")]
use std::sync::Mutex;
#[cfg(feature = "gamepad")]
use std::time::{Duration, Instant};

use super::controller_tips::TipToken;
use TipToken::{Button, Plus, Text};

pub const FADE_SECS: f32 = 0.15;

static SHOWN: AtomicBool = AtomicBool::new(false);

pub const HINT_RETIRE_OPENS: u32 = 3;

#[cfg(feature = "gamepad")]
static HINT_RETIRED: Mutex<Option<(Instant, bool)>> = Mutex::new(None);

pub fn set_shown(on: bool) {
    if SHOWN.swap(on, Ordering::SeqCst) != on {
        super::vk_ui::request_repaint();
        if on && sheet_open_counts() {
            let _ = std::thread::Builder::new()
                .name("sheet-opens".into())
                .spawn(record_open);
        }
    }
}

fn sheet_open_counts() -> bool {
    #[cfg(all(feature = "gamepad", not(test)))]
    {
        super::vk_ui::vk_look().tv_layout() && !hint_retired()
    }
    #[cfg(not(all(feature = "gamepad", not(test))))]
    {
        true
    }
}

fn record_open() {
    #[cfg(all(feature = "gamepad", not(test)))]
    {
        if !super::vk_ui::vk_look().tv_layout() {
            return;
        }
        let opens = crate::config::gamepad_settings().vk_sheet_opens;
        if opens < HINT_RETIRE_OPENS {
            let _ = crate::config::set_gamepad_setting("vk_sheet_opens", &(opens + 1).to_string());
            *HINT_RETIRED.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }
}

pub fn hint_retired() -> bool {
    #[cfg(feature = "gamepad")]
    {
        let mut cache = HINT_RETIRED.lock().unwrap_or_else(|e| e.into_inner());
        match *cache {
            Some((at, value)) if at.elapsed() < Duration::from_millis(250) => value,
            _ => {
                let value = crate::config::gamepad_settings().vk_sheet_opens >= HINT_RETIRE_OPENS;
                *cache = Some((Instant::now(), value));
                value
            }
        }
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
            row(
                &[Button("SELECT"), Plus, Button("START")],
                "Controller Center",
            ),
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

pub static SHEET_HINT: SheetRow = row(&[Text("Hold"), Button("SELECT")], "All shortcuts");

#[cfg(not(feature = "vk-panels"))]
const MODERN_EDITING: &[SheetRow] = &[
    row(&[Button("SELECT"), Plus, Button("X")], "Copy"),
    row(&[Button("SELECT"), Plus, Button("Y")], "Paste"),
    row(&[Button("SELECT"), Plus, Button("B")], "Clear field"),
];

#[cfg(feature = "vk-panels")]
const MODERN_EDITING: &[SheetRow] = &[
    row(&[Button("SELECT"), Plus, Button("X")], "Copy"),
    row(&[Button("SELECT"), Plus, Button("Y")], "Paste"),
    row(&[Button("SELECT"), Plus, Button("A")], "Clipboard & emoji"),
    row(&[Button("SELECT"), Plus, Button("B")], "Clear field"),
];

pub static MODERN_GROUPS: [SheetGroup; 6] = [
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
        rows: MODERN_EDITING,
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
            row(
                &[Button("SELECT"), Plus, Button("START")],
                "Controller Center",
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

pub static LEGEND: [SheetRow; 9] = [
    row(&[Button("A")], "Type"),
    row(&[Button("B")], "Delete"),
    row(&[Button("LB"), SLASH, Button("RB")], "Move caret"),
    row(&[Button("RT")], "Shift"),
    row(&[Button("R3")], "Dictate"),
    row(&[Button("SELECT"), Plus, Button("Y")], "Paste"),
    row(&[Text("Tap"), Button("SELECT")], "Suggestions"),
    row(&[Button("L3")], "Close keyboard"),
    row(&[Text("Hold"), Button("SELECT")], "All shortcuts"),
];

fn show_voice_shortcut() -> bool {
    crate::config::voice_enabled()
}

/// Legend rows the keyboard actually draws. Dictate is omitted while voice
/// typing is off, so the bar does not advertise a dead button.
pub fn shown_legend() -> Vec<SheetRow> {
    LEGEND
        .iter()
        .copied()
        .filter(|row| row.label != "Dictate" || show_voice_shortcut())
        .collect()
}

pub fn shown_rows(group: &SheetGroup) -> Vec<SheetRow> {
    group
        .rows
        .iter()
        .copied()
        .filter(|row| row.label != "Dictate" || show_voice_shortcut())
        .collect()
}

/// Shortcut sheet columns with the Dictate row removed while voice typing is off.
pub fn shown_columns() -> [Vec<Line>; 3] {
    let mut cols = columns();
    if !show_voice_shortcut() {
        for col in &mut cols {
            col.retain(|line| match line {
                Line::Row(row) => row.label != "Dictate",
                _ => true,
            });
        }
    }
    cols
}

pub fn shown_line_count() -> usize {
    shown_columns()
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(1)
        .max(1)
}

pub fn fit_legend(widths: &[f32], sep: f32, avail: f32) -> Vec<usize> {
    let mut kept: Vec<usize> = (0..widths.len()).collect();
    let total = |kept: &[usize]| {
        kept.iter().map(|&i| widths[i]).sum::<f32>() + sep * kept.len().saturating_sub(1) as f32
    };
    while kept.len() > 1 && total(&kept) > avail {
        kept.remove((kept.len() - 1) / 2);
    }
    kept
}

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

pub fn hairline_below(col: &[Line], i: usize) -> bool {
    matches!(col.get(i), Some(Line::Row(_))) && matches!(col.get(i + 1), Some(Line::Row(_)))
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
        .chain(MODERN_GROUPS.iter())
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
    fn legend_fit_keeps_everything_when_it_fits() {
        let widths = [10.0; 10];
        assert_eq!(fit_legend(&widths, 2.0, 118.0), (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn legend_fit_drops_from_the_middle_and_keeps_the_last() {
        let widths = [10.0; 10];
        assert_eq!(fit_legend(&widths, 2.0, 117.0), [0, 1, 2, 3, 5, 6, 7, 8, 9]);
        assert_eq!(fit_legend(&widths, 2.0, 94.0), [0, 1, 2, 3, 6, 7, 8, 9]);
        assert_eq!(fit_legend(&widths, 2.0, 81.0), [0, 1, 2, 7, 8, 9]);
        assert_eq!(fit_legend(&widths, 2.0, 5.0), [9]);
        assert_eq!(fit_legend(&[], 2.0, 5.0), Vec::<usize>::new());
    }

    #[test]
    fn legend_rows_match_the_sheet_and_the_vk_bindings() {
        let sheet: Vec<&SheetRow> = GROUPS[0].rows.iter().collect();
        for r in &LEGEND[2..8] {
            assert!(sheet.contains(&r), "{}", r.label);
        }
        assert_eq!(LEGEND[0].keys, &[Button("A")]);
        assert_eq!(LEGEND[1].keys, &[Button("B")]);
        let last = LEGEND[LEGEND.len() - 1];
        assert_eq!(last.label, "All shortcuts");
        assert_eq!(last.keys, &[Text("Hold"), Button("SELECT")]);
    }

    #[test]
    fn bar_drops_symbols_but_the_sheet_keeps_it() {
        assert!(LEGEND.iter().all(|r| r.label != "Symbols"));
        assert!(GROUPS[0].rows.iter().any(|r| r.label == "Symbols"));
        let labels: Vec<&str> = LEGEND.iter().map(|r| r.label).collect();
        assert_eq!(
            labels,
            [
                "Type",
                "Delete",
                "Move caret",
                "Shift",
                "Dictate",
                "Paste",
                "Suggestions",
                "Close keyboard",
                "All shortcuts"
            ]
        );
    }

    #[test]
    fn hairlines_skip_headers_and_the_last_row_of_each_group() {
        let cols = columns();
        let without: Vec<&str> = cols
            .iter()
            .flat_map(|c| {
                (0..c.len()).filter_map(move |i| match c[i] {
                    Line::Row(r) if !hairline_below(c, i) => Some(r.label),
                    _ => None,
                })
            })
            .collect();
        assert_eq!(
            without,
            [
                "Clear field",
                "Controller Center",
                "Open warmUP",
                "Record (warmUP)"
            ]
        );
        for c in &cols {
            for (i, l) in c.iter().enumerate() {
                if !matches!(l, Line::Row(_)) {
                    assert!(!hairline_below(c, i));
                }
            }
        }
        let firsts: Vec<Line> = cols.iter().map(|c| c[0]).collect();
        assert_eq!(
            firsts,
            [
                Line::Title("Keyboard"),
                Line::Blank,
                Line::Title("Keyboard closed")
            ]
        );
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
            "Controller Center",
            "Record (warmUP)",
        ] {
            assert!(labels.contains(&l), "{l}");
        }
        let paste = GROUPS[0].rows.iter().find(|r| r.label == "Paste").unwrap();
        assert_eq!(paste.keys, &[Button("SELECT"), Plus, Button("Y")]);
    }

    #[test]
    fn groups_are_sorted_into_six_tiles() {
        let titles: Vec<&str> = MODERN_GROUPS.iter().map(|g| g.title).collect();
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
        assert!(MODERN_GROUPS.iter().all(|g| !g.rows.is_empty()));
    }

    #[test]
    fn hint_points_at_the_sheet() {
        assert_eq!(SHEET_HINT.label, "All shortcuts");
        assert_eq!(SHEET_HINT.keys, &[Text("Hold"), Button("SELECT")]);
    }

    #[test]
    fn modern_sheet_rows_match_the_mapped_shortcuts() {
        let labels: Vec<&str> = MODERN_GROUPS
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
            "Controller Center",
            "Record (warmUP)",
        ] {
            assert!(labels.contains(&l), "{l}");
        }
        #[cfg(feature = "vk-panels")]
        assert!(labels.contains(&"Clipboard & emoji"));
        let paste = MODERN_GROUPS[1]
            .rows
            .iter()
            .find(|r| r.label == "Paste")
            .unwrap();
        assert_eq!(paste.keys, &[Button("SELECT"), Plus, Button("Y")]);
    }
}
