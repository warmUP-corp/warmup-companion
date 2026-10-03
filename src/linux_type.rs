use std::io::{Read, Write};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use evdev::KeyCode;

pub const INJECT_CHUNK: usize = 8;
pub const INJECT_GAP: Duration = Duration::from_millis(6);

struct Board {
    device: evdev::uinput::VirtualDevice,
}

static BOARD: OnceLock<Mutex<Option<Board>>> = OnceLock::new();

fn board() -> &'static Mutex<Option<Board>> {
    BOARD.get_or_init(|| Mutex::new(None))
}

fn with_board<T>(body: impl FnOnce(&mut Board) -> T) -> Result<T, String> {
    let mut slot = board().lock().map_err(|error| error.to_string())?;
    if slot.is_none() {
        let device = open_keyboard()?;
        wait_for_device();
        *slot = Some(Board { device });
    }
    let board = slot.as_mut().expect("keyboard");
    Ok(body(board))
}

pub fn tap_vk(vk: u16) -> bool {
    let Some(code) = evdev_from_vk(vk) else {
        crate::install::log_line(&format!("text inject: no key for vk {vk:#x}"));
        return false;
    };
    tap_code(code)
}

pub fn tap_ctrl(vk: u16) -> bool {
    let Some(code) = evdev_from_vk(vk) else {
        return false;
    };
    chord(KeyCode::KEY_LEFTCTRL, code)
}

pub fn ctrl_then(ctrl_vk: u16, vk: u16) -> bool {
    tap_ctrl(ctrl_vk) && tap_vk(vk)
}

pub fn type_text(text: &str) {
    let mut layout = Layout::load();
    let chars: Vec<char> = text.chars().collect();
    for (index, ch) in chars.iter().copied().enumerate() {
        if !type_char(ch, &mut layout) {
            crate::install::log_line(&format!("text inject: could not type {ch:?}"));
        }
        if index + 1 < chars.len() && (index + 1) % INJECT_CHUNK.max(1) == 0 {
            std::thread::sleep(INJECT_GAP);
        }
    }
}

pub fn replace_text(del: usize, ins: &str) -> std::io::Result<()> {
    for _ in 0..del {
        if !tap_vk(0x08) {
            return Err(std::io::Error::other("backspace failed"));
        }
    }
    type_text(ins);
    Ok(())
}

fn type_char(ch: char, layout: &mut Layout) -> bool {
    if ch == '\n' || ch == '\r' {
        return tap_vk(0x0D);
    }
    if ch == '\u{8}' {
        return tap_vk(0x08);
    }
    if let Some((code, level)) = layout.lookup(ch) {
        return emit_level(code, level);
    }
    crate::install::log_line(&format!(
        "text inject: {ch:?} is outside the active layout"
    ));
    virtual_keyboard::type_char(ch)
}

fn tap_code(code: KeyCode) -> bool {
    match with_board(|board| {
        let _ = emit(&mut board.device, code, 1);
        let _ = emit(&mut board.device, code, 0);
    }) {
        Ok(()) => true,
        Err(error) => {
            crate::install::log_line(&format!("text inject: {error}"));
            false
        }
    }
}

fn chord(modifier: KeyCode, code: KeyCode) -> bool {
    match with_board(|board| {
        let _ = emit(&mut board.device, modifier, 1);
        let _ = emit(&mut board.device, code, 1);
        let _ = emit(&mut board.device, code, 0);
        let _ = emit(&mut board.device, modifier, 0);
    }) {
        Ok(()) => true,
        Err(error) => {
            crate::install::log_line(&format!("text inject: {error}"));
            false
        }
    }
}

fn emit_level(code: KeyCode, level: u32) -> bool {
    let shift = level == 1 || level == 3;
    let altgr = level == 2 || level == 3;
    match with_board(|board| {
        if shift {
            let _ = emit(&mut board.device, KeyCode::KEY_LEFTSHIFT, 1);
        }
        if altgr {
            let _ = emit(&mut board.device, KeyCode::KEY_RIGHTALT, 1);
        }
        let _ = emit(&mut board.device, code, 1);
        let _ = emit(&mut board.device, code, 0);
        if altgr {
            let _ = emit(&mut board.device, KeyCode::KEY_RIGHTALT, 0);
        }
        if shift {
            let _ = emit(&mut board.device, KeyCode::KEY_LEFTSHIFT, 0);
        }
    }) {
        Ok(()) => true,
        Err(error) => {
            crate::install::log_line(&format!("text inject: {error}"));
            false
        }
    }
}

fn emit(device: &mut evdev::uinput::VirtualDevice, code: KeyCode, value: i32) -> std::io::Result<()> {
    let event = evdev::InputEvent::from(evdev::KeyEvent::new_now(code, value));
    device.emit(&[event])
}

fn open_keyboard() -> Result<evdev::uinput::VirtualDevice, String> {
    use evdev::{AttributeSet, BusType, InputId, KeyCode};
    let mut keys = AttributeSet::<KeyCode>::new();
    for code in KEYS {
        keys.insert(*code);
    }
    evdev::uinput::VirtualDevice::builder()
        .map_err(open_error)?
        .name("warmup-companion text")
        .input_id(InputId::new(BusType::BUS_VIRTUAL, 0x1234, 0x5757, 1))
        .with_keys(&keys)
        .map_err(open_error)?
        .build()
        .map_err(open_error)
}

fn open_error(error: std::io::Error) -> String {
    format!("text keyboard: {error}")
}

fn wait_for_device() {
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    while std::time::Instant::now() < deadline {
        let listed = std::fs::read_to_string("/proc/bus/input/devices").unwrap_or_default();
        if listed.contains("warmup-companion text") {
            std::thread::sleep(Duration::from_millis(40));
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn evdev_from_vk(vk: u16) -> Option<KeyCode> {
    use KeyCode as K;
    Some(match vk {
        0x08 => K::KEY_BACKSPACE,
        0x09 => K::KEY_TAB,
        0x0D => K::KEY_ENTER,
        0x11 => K::KEY_LEFTCTRL,
        0x1B => K::KEY_ESC,
        0x20 => K::KEY_SPACE,
        0x23 => K::KEY_END,
        0x25 => K::KEY_LEFT,
        0x26 => K::KEY_UP,
        0x27 => K::KEY_RIGHT,
        0x28 => K::KEY_DOWN,
        0x30 => K::KEY_0,
        0x31 => K::KEY_1,
        0x32 => K::KEY_2,
        0x33 => K::KEY_3,
        0x34 => K::KEY_4,
        0x35 => K::KEY_5,
        0x36 => K::KEY_6,
        0x37 => K::KEY_7,
        0x38 => K::KEY_8,
        0x39 => K::KEY_9,
        0x41 => K::KEY_A,
        0x42 => K::KEY_B,
        0x43 => K::KEY_C,
        0x44 => K::KEY_D,
        0x45 => K::KEY_E,
        0x46 => K::KEY_F,
        0x47 => K::KEY_G,
        0x48 => K::KEY_H,
        0x49 => K::KEY_I,
        0x4A => K::KEY_J,
        0x4B => K::KEY_K,
        0x4C => K::KEY_L,
        0x4D => K::KEY_M,
        0x4E => K::KEY_N,
        0x4F => K::KEY_O,
        0x50 => K::KEY_P,
        0x51 => K::KEY_Q,
        0x52 => K::KEY_R,
        0x53 => K::KEY_S,
        0x54 => K::KEY_T,
        0x55 => K::KEY_U,
        0x56 => K::KEY_V,
        0x57 => K::KEY_W,
        0x58 => K::KEY_X,
        0x59 => K::KEY_Y,
        0x5A => K::KEY_Z,
        _ => return None,
    })
}

const KEYS: &[KeyCode] = &[
    KeyCode::KEY_ESC,
    KeyCode::KEY_1,
    KeyCode::KEY_2,
    KeyCode::KEY_3,
    KeyCode::KEY_4,
    KeyCode::KEY_5,
    KeyCode::KEY_6,
    KeyCode::KEY_7,
    KeyCode::KEY_8,
    KeyCode::KEY_9,
    KeyCode::KEY_0,
    KeyCode::KEY_MINUS,
    KeyCode::KEY_EQUAL,
    KeyCode::KEY_BACKSPACE,
    KeyCode::KEY_TAB,
    KeyCode::KEY_Q,
    KeyCode::KEY_W,
    KeyCode::KEY_E,
    KeyCode::KEY_R,
    KeyCode::KEY_T,
    KeyCode::KEY_Y,
    KeyCode::KEY_U,
    KeyCode::KEY_I,
    KeyCode::KEY_O,
    KeyCode::KEY_P,
    KeyCode::KEY_LEFTBRACE,
    KeyCode::KEY_RIGHTBRACE,
    KeyCode::KEY_ENTER,
    KeyCode::KEY_LEFTCTRL,
    KeyCode::KEY_A,
    KeyCode::KEY_S,
    KeyCode::KEY_D,
    KeyCode::KEY_F,
    KeyCode::KEY_G,
    KeyCode::KEY_H,
    KeyCode::KEY_J,
    KeyCode::KEY_K,
    KeyCode::KEY_L,
    KeyCode::KEY_SEMICOLON,
    KeyCode::KEY_APOSTROPHE,
    KeyCode::KEY_GRAVE,
    KeyCode::KEY_LEFTSHIFT,
    KeyCode::KEY_BACKSLASH,
    KeyCode::KEY_Z,
    KeyCode::KEY_X,
    KeyCode::KEY_C,
    KeyCode::KEY_V,
    KeyCode::KEY_B,
    KeyCode::KEY_N,
    KeyCode::KEY_M,
    KeyCode::KEY_COMMA,
    KeyCode::KEY_DOT,
    KeyCode::KEY_SLASH,
    KeyCode::KEY_RIGHTSHIFT,
    KeyCode::KEY_LEFTALT,
    KeyCode::KEY_SPACE,
    KeyCode::KEY_RIGHTALT,
    KeyCode::KEY_UP,
    KeyCode::KEY_DOWN,
    KeyCode::KEY_LEFT,
    KeyCode::KEY_RIGHT,
    KeyCode::KEY_END,
    KeyCode::KEY_HOME,
    KeyCode::KEY_DELETE,
    KeyCode::KEY_102ND,
];

struct Layout {
    map: Vec<(u32, KeyCode, u32)>,
}

impl Layout {
    fn load() -> Self {
        let mut map = Vec::new();
        if let Some(keymap) = load_keymap() {
            let min = keymap.min_keycode().raw();
            let max = keymap.max_keycode().raw();
            for raw in min..=max {
                let key = xkbcommon::xkb::Keycode::new(raw);
                let layouts = keymap.num_layouts_for_key(key).max(1);
                for layout in 0..layouts {
                    let levels = keymap.num_levels_for_key(key, layout);
                    for level in 0..levels.min(4) {
                        for sym in keymap.key_get_syms_by_level(key, layout, level) {
                            let ch = xkbcommon::xkb::keysym_to_utf32(*sym);
                            if ch == 0 || raw < 8 {
                                continue;
                            }
                            let evdev = (raw - 8) as u16;
                            let Some(code) = code_for_raw(evdev) else {
                                continue;
                            };
                            if !map.iter().any(|(have, _, _)| *have == ch) {
                                map.push((ch, code, level));
                            }
                        }
                    }
                }
            }
        }
        Self { map }
    }

    fn lookup(&self, ch: char) -> Option<(KeyCode, u32)> {
        let want = ch as u32;
        self.map
            .iter()
            .find(|(have, _, _)| *have == want)
            .map(|(_, code, level)| (*code, *level))
    }
}

fn load_keymap() -> Option<xkbcommon::xkb::Keymap> {
    let context = xkbcommon::xkb::Context::new(xkbcommon::xkb::CONTEXT_NO_FLAGS);
    let layout = active_layout();
    xkbcommon::xkb::Keymap::new_from_names(
        &context,
        "evdev",
        "pc105",
        &layout,
        "",
        None,
        xkbcommon::xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .or_else(|| {
        xkbcommon::xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            "",
            "",
            None,
            xkbcommon::xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
    })
}

fn active_layout() -> String {
    if let Ok(layout) = std::env::var("XKB_DEFAULT_LAYOUT") {
        if !layout.is_empty() {
            return layout;
        }
    }
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        if let Some(layout) = hypr_layout() {
            return layout;
        }
    }
    "us".to_string()
}

fn hypr_layout() -> Option<String> {
    let sig = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let sock = std::path::PathBuf::from(runtime)
        .join("hypr")
        .join(sig)
        .join(".socket.sock");
    let mut stream = std::os::unix::net::UnixStream::connect(sock).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(300)))
        .ok()?;
    stream.write_all(b"j/getoption input:kb_layout").ok()?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let value = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    let layout = value.get("str")?.as_str()?.trim();
    if layout.is_empty() {
        None
    } else {
        Some(layout.split(',').next().unwrap_or(layout).to_string())
    }
}

fn code_for_raw(code: u16) -> Option<KeyCode> {
    KEYS.iter().copied().find(|key| key.0 == code)
}

mod virtual_keyboard {
    use std::os::fd::AsFd;
    use std::sync::Mutex;
    use std::time::Instant;

    use wayland_client::protocol::wl_seat;
    use wayland_client::{
        Connection, Dispatch, QueueHandle,
        globals::{GlobalListContents, registry_queue_init},
    };
    use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
        zwp_virtual_keyboard_manager_v1, zwp_virtual_keyboard_v1,
    };

    struct Seat;

    impl Dispatch<wl_seat::WlSeat, ()> for Seat {
        fn event(
            _: &mut Self,
            _: &wl_seat::WlSeat,
            _: wl_seat::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1, ()> for Seat {
        fn event(
            _: &mut Self,
            _: &zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
            _: zwp_virtual_keyboard_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, ()> for Seat {
        fn event(
            _: &mut Self,
            _: &zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
            _: zwp_virtual_keyboard_manager_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<wayland_client::protocol::wl_registry::WlRegistry, GlobalListContents> for Seat {
        fn event(
            _: &mut Self,
            _: &wayland_client::protocol::wl_registry::WlRegistry,
            _: wayland_client::protocol::wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    struct Client {
        queue: wayland_client::EventQueue<Seat>,
        keyboard: zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
        started: Instant,
    }

    static CLIENT: Mutex<Option<Client>> = Mutex::new(None);

    pub fn type_char(ch: char) -> bool {
        let mut slot = match CLIENT.lock() {
            Ok(slot) => slot,
            Err(_) => return false,
        };
        if slot.is_none() {
            match connect() {
                Ok(client) => *slot = Some(client),
                Err(error) => {
                    crate::install::log_line(&format!(
                        "virtual keyboard unavailable for {ch:?}: {error}"
                    ));
                    return false;
                }
            }
        }
        let keymap = keymap_text(ch);
        let Some(file) = keymap_file(&keymap) else {
            return false;
        };
        let failed = {
            let Some(client) = slot.as_mut() else {
                return false;
            };
            let time = client.started.elapsed().as_millis() as u32;
            client
                .keyboard
                .keymap(1, file.as_fd(), keymap.len() as u32);
            client.keyboard.modifiers(0, 0, 0, 0);
            client.keyboard.key(time, 30, 1);
            client.keyboard.key(time.wrapping_add(8), 30, 0);
            let mut data = Seat;
            client.queue.roundtrip(&mut data).is_err()
        };
        if failed {
            *slot = None;
            return false;
        }
        true
    }

    fn connect() -> Result<Client, String> {
        let conn = Connection::connect_to_env().map_err(|error| error.to_string())?;
        let (globals, mut queue) =
            registry_queue_init::<Seat>(&conn).map_err(|error| error.to_string())?;
        let qh = queue.handle();
        let seat = globals
            .bind::<wl_seat::WlSeat, _, _>(&qh, 1..=7, ())
            .map_err(|error| format!("seat: {error}"))?;
        let manager = globals
            .bind::<zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, _, _>(
                &qh,
                1..=1,
                (),
            )
            .map_err(|_| "zwp_virtual_keyboard_manager_v1 is not advertised".to_string())?;
        let keyboard = manager.create_virtual_keyboard(&seat, &qh, ());
        let mut data = Seat;
        queue
            .roundtrip(&mut data)
            .map_err(|error| error.to_string())?;
        Ok(Client {
            queue,
            keyboard,
            started: Instant::now(),
        })
    }

    fn keymap_text(ch: char) -> String {
        format!(
            "xkb_keymap {{\n\
xkb_keycodes \"(unnamed)\" {{\n\
minimum = 8;\n\
maximum = 255;\n\
<K38> = 38;\n\
}};\n\
xkb_types \"(unnamed)\" {{ include \"complete\" }};\n\
xkb_compatibility \"(unnamed)\" {{ include \"complete\" }};\n\
xkb_symbols \"(unnamed)\" {{\n\
key <K38> {{ [ U{cp:04X} ] }};\n\
}};\n\
}};\n",
            cp = ch as u32
        )
    }

    fn keymap_file(text: &str) -> Option<std::fs::File> {
        let dir = crate::paths::runtime_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("vk-keymap");
        std::fs::write(&path, text).ok()?;
        std::fs::File::open(path).ok()
    }

    #[cfg(test)]
    mod tests {
        use super::keymap_text;

        #[test]
        fn one_off_keymap_compiles() {
            let text = keymap_text('α');
            let context = xkbcommon::xkb::Context::new(xkbcommon::xkb::CONTEXT_NO_FLAGS);
            let keymap = xkbcommon::xkb::Keymap::new_from_string(
                &context,
                text,
                xkbcommon::xkb::KEYMAP_FORMAT_TEXT_V1,
                xkbcommon::xkb::KEYMAP_COMPILE_NO_FLAGS,
            );
            assert!(keymap.is_some());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::evdev_from_vk;
    use evdev::KeyCode;

    #[test]
    fn letters_and_controls_map_to_evdev() {
        assert_eq!(evdev_from_vk(0x41), Some(KeyCode::KEY_A));
        assert_eq!(evdev_from_vk(0x08), Some(KeyCode::KEY_BACKSPACE));
        assert_eq!(evdev_from_vk(0x0D), Some(KeyCode::KEY_ENTER));
        assert_eq!(evdev_from_vk(0x25), Some(KeyCode::KEY_LEFT));
        assert_eq!(evdev_from_vk(0x31), Some(KeyCode::KEY_1));
    }

    #[test]
    fn active_layout_contains_plain_letters() {
        let layout = super::Layout::load();
        assert!(layout.lookup('a').is_some());
        assert!(layout.lookup('A').is_some());
    }
}
