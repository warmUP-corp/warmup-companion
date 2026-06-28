//! Warmup-owned Xbox VK (`WarmupXboxVkWindow`).

use std::fmt;
use std::time::Duration;

use super::vk_ui::{self, VkAttach, VkUiThread};

pub struct VkSession {
    ui: Option<VkUiThread>,
}

impl fmt::Debug for VkSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VkSession")
            .field("backend", &self.describe())
            .finish()
    }
}

impl VkSession {
    pub fn open(attach: VkAttach) -> Result<Self, String> {
        if matches!(attach, VkAttach::Input) {
            super::native_keyboard::suppress_for(Duration::from_millis(2000));
        }
        let ui = VkUiThread::spawn(attach)?;
        ui.show(attach)?;
        if !vk_ui::wait_until_visible(Duration::from_secs(5)) {
            let _ = ui.hide();
            return Err("VK window did not become visible (desktop attach or Session 0 UI)".into());
        }
        Ok(Self { ui: Some(ui) })
    }

    pub fn describe(&self) -> &'static str {
        "WarmupXboxVkWindow (native UI)"
    }

    pub fn close(mut self) {
        if let Some(ui) = self.ui.take() {
            let _ = ui.hide();
        }
    }
}

impl Drop for VkSession {
    fn drop(&mut self) {
        if let Some(ui) = self.ui.take() {
            let _ = ui.hide();
        }
    }
}
