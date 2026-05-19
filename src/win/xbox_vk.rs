//! Warmup-owned Xbox VK (`WarmupXboxVkWindow`).

use std::fmt;
use std::sync::OnceLock;
use std::time::Duration;

use super::vk_ui::{self, VkAttach, VkUiThread};

static VK_UI: OnceLock<VkUiThread> = OnceLock::new();

pub struct VkSession;

impl fmt::Debug for VkSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VkSession")
            .field("backend", &self.describe())
            .finish()
    }
}

impl VkSession {
    pub fn open(attach: VkAttach) -> Result<Self, String> {
        ui()?.show(attach)?;
        if !vk_ui::wait_until_visible(Duration::from_millis(750)) {
            let _ = ui().map(|u| u.hide());
            return Err(
                "VK window did not become visible (desktop attach or Session 0 UI)".into(),
            );
        }
        Ok(Self)
    }

    pub fn describe(&self) -> &'static str {
        "WarmupXboxVkWindow (native UI)"
    }

    pub fn close(self) {
        if let Some(ui) = VK_UI.get() {
            let _ = ui.hide();
        }
    }
}

impl Drop for VkSession {
    fn drop(&mut self) {
        if let Some(ui) = VK_UI.get() {
            let _ = ui.hide();
        }
    }
}

fn ui() -> Result<&'static VkUiThread, String> {
    if let Some(ui) = VK_UI.get() {
        return Ok(ui);
    }
    let ui = VkUiThread::spawn()?;
    let _ = VK_UI.set(ui);
    VK_UI.get()
        .ok_or_else(|| "vk ui thread failed to initialize".to_string())
}
