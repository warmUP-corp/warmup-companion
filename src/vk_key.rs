#[cfg(windows)]
pub use windows::Win32::UI::Input::KeyboardAndMouse::{
    VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DOWN, VK_END, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT,
    VK_SPACE, VK_UP,
};

#[cfg(not(windows))]
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VIRTUAL_KEY(pub u16);

#[cfg(not(windows))]
pub const VK_BACK: VIRTUAL_KEY = VIRTUAL_KEY(0x08);
#[cfg(not(windows))]
pub const VK_RETURN: VIRTUAL_KEY = VIRTUAL_KEY(0x0D);
#[cfg(not(windows))]
pub const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
#[cfg(not(windows))]
pub const VK_ESCAPE: VIRTUAL_KEY = VIRTUAL_KEY(0x1B);
#[cfg(not(windows))]
pub const VK_SPACE: VIRTUAL_KEY = VIRTUAL_KEY(0x20);
#[cfg(not(windows))]
pub const VK_END: VIRTUAL_KEY = VIRTUAL_KEY(0x23);
#[cfg(not(windows))]
pub const VK_LEFT: VIRTUAL_KEY = VIRTUAL_KEY(0x25);
#[cfg(not(windows))]
pub const VK_UP: VIRTUAL_KEY = VIRTUAL_KEY(0x26);
#[cfg(not(windows))]
pub const VK_RIGHT: VIRTUAL_KEY = VIRTUAL_KEY(0x27);
#[cfg(not(windows))]
pub const VK_DOWN: VIRTUAL_KEY = VIRTUAL_KEY(0x28);
