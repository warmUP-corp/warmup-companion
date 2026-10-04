use std::sync::atomic::{AtomicBool, Ordering};

static SHOWN: AtomicBool = AtomicBool::new(false);

pub fn shown() -> bool {
    SHOWN.load(Ordering::SeqCst)
}

pub fn set_shown(on: bool) {
    if SHOWN.swap(on, Ordering::SeqCst) == on {
        return;
    }
    #[cfg(all(target_os = "linux", feature = "gamepad"))]
    crate::linux_vk::request_repaint();
}
