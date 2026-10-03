pub const TABS: [&str; 4] = ["General", "Mouse", "Keyboard", "Controller"];

#[derive(Clone, Copy)]
pub enum RowKind {
    Check {
        key: &'static str,
        on_val: &'static str,
        off_val: &'static str,
    },
    PausePoll,
    Slider {
        key: &'static str,
        min: i32,
        max: i32,
        div: f32,
        decimals: usize,
    },
    Vocabulary,
}

pub struct Row {
    pub tab: usize,
    pub label: &'static str,
    pub kind: RowKind,
}

const fn check(tab: usize, label: &'static str, key: &'static str) -> Row {
    Row {
        tab,
        label,
        kind: RowKind::Check {
            key,
            on_val: "true",
            off_val: "false",
        },
    }
}

const fn mapped(
    tab: usize,
    label: &'static str,
    key: &'static str,
    on_val: &'static str,
    off_val: &'static str,
) -> Row {
    Row {
        tab,
        label,
        kind: RowKind::Check {
            key,
            on_val,
            off_val,
        },
    }
}

const fn slider(
    tab: usize,
    label: &'static str,
    key: &'static str,
    min: i32,
    max: i32,
    div: f32,
    decimals: usize,
) -> Row {
    Row {
        tab,
        label,
        kind: RowKind::Slider {
            key,
            min,
            max,
            div,
            decimals,
        },
    }
}

pub static ROWS: &[Row] = &[
    check(
        0,
        "Enable gamepad cursor (sticks move the mouse, A/B click)",
        "cursor_enabled",
    ),
    check(
        0,
        "Sleep during games (only the Guide button is watched)",
        "sleep_on_game",
    ),
    check(0, "Guide-only in games (legacy)", "auto_stop_on_game"),
    check(
        0,
        "Show controller hints on the sign-in screen",
        "signin_hints",
    ),
    check(
        0,
        "Guide / PS button opens warmUP when it is closed",
        "guide_launch",
    ),
    Row {
        tab: 0,
        label: "Pause gamepad input",
        kind: RowKind::PausePoll,
    },
    mapped(
        0,
        "Only run on the sign-in and lock screen (sleeps after you sign in)",
        "run_mode",
        "signin",
        "always",
    ),
    slider(1, "Cursor speed", "cursor_speed", 1, 40, 1.0, 0),
    slider(1, "Cursor acceleration", "cursor_accel", 10, 50, 10.0, 1),
    slider(1, "Cursor dead zone", "cursor_deadzone", 0, 90, 100.0, 2),
    slider(1, "Cursor smoothing", "cursor_smoothing", 0, 90, 100.0, 2),
    slider(1, "Scroll speed", "scroll_speed", 1, 20, 1.0, 0),
    slider(1, "Scroll acceleration", "scroll_accel", 10, 50, 10.0, 1),
    check(
        1,
        "Natural scrolling (invert scroll direction)",
        "natural_scroll",
    ),
    mapped(
        2,
        "Floating layout (card in the middle instead of a docked bar)",
        "vk_mode",
        "floating",
        "docked",
    ),
    mapped(
        2,
        "Compact size (smaller docked bar)",
        "vk_bar_scale",
        "0.8",
        "1.0",
    ),
    Row {
        tab: 2,
        label: "Coding vocabulary (terminal & agent terms)",
        kind: RowKind::Vocabulary,
    },
];
