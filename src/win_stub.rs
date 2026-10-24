//! Non-Windows stubs.

pub mod desktop {
    pub fn attach_named(_name: &str) -> Result<(), String> {
        Err("real desktop attach requires Windows".into())
    }
    pub fn attach_input() -> Result<(), String> {
        Err("real desktop attach requires Windows".into())
    }
    pub fn current_desktop_name() -> Option<String> {
        None
    }
    pub fn input_desktop_name() -> Result<String, String> {
        Err("real desktop attach requires Windows".into())
    }
}

pub mod xbox_vk {
    pub struct VkSession {
        #[cfg(all(target_os = "linux", feature = "gamepad"))]
        overlay: Option<crate::linux_vk::Overlay>,
    }

    pub enum VkAttach {
        Current,
        Input,
    }

    impl VkSession {
        pub fn open(attach: VkAttach) -> Result<Self, String> {
            #[cfg(all(target_os = "linux", feature = "gamepad"))]
            {
                let overlay = crate::linux_vk::Overlay::open(matches!(attach, VkAttach::Input))?;
                return Ok(Self { overlay: Some(overlay) });
            }
            #[cfg(not(all(target_os = "linux", feature = "gamepad")))]
            {
                let _ = attach;
                Err("real on-screen keyboard requires Windows".into())
            }
        }
        pub fn describe(&self) -> &'static str {
            #[cfg(all(target_os = "linux", feature = "gamepad"))]
            {
                "layer shell keyboard"
            }
            #[cfg(not(all(target_os = "linux", feature = "gamepad")))]
            {
                "n/a"
            }
        }
        pub fn close(mut self) {
            #[cfg(all(target_os = "linux", feature = "gamepad"))]
            if let Some(overlay) = self.overlay.take() {
                overlay.close();
            }
        }
    }

    impl std::fmt::Debug for VkSession {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("VkSession")
        }
    }

    impl Drop for VkSession {
        fn drop(&mut self) {
            #[cfg(all(target_os = "linux", feature = "gamepad"))]
            if let Some(overlay) = self.overlay.take() {
                overlay.close();
            }
        }
    }
}

#[cfg(feature = "gamepad")]
#[path = "win/controller_center.rs"]
pub mod controller_center;

#[cfg(feature = "gamepad")]
#[path = "win/tray_menu.rs"]
pub(crate) mod tray_menu;

#[cfg(feature = "gamepad")]
#[path = "win/update_window.rs"]
pub(crate) mod update_window;

#[cfg(feature = "gamepad")]
pub(crate) mod vk_renderer {
    pub(crate) use crate::vk_paint::is_playstation_label;
}

#[allow(dead_code)]
#[path = "win/controller_tips.rs"]
pub mod controller_tips;

#[allow(dead_code)]
#[path = "win/shortcut_sheet.rs"]
pub mod shortcut_sheet;

#[allow(dead_code)]
pub mod vk_ui {
    pub fn request_repaint() {
        #[cfg(feature = "gamepad")]
        crate::linux_vk::request_repaint();
    }

    pub(crate) fn vk_look() -> crate::vk_paint::VkLook {
        #[cfg(feature = "gamepad")]
        {
            crate::linux_vk::look()
        }
        #[cfg(not(feature = "gamepad"))]
        {
            crate::vk_paint::VkLook {
                style: crate::config::VkStyle::Normal,
                tv: false,
            }
        }
    }
}

pub mod speech_input {
    #[cfg_attr(not(feature = "gamepad"), allow(dead_code))]
    pub fn set_engine(_name: &str) {}

    #[cfg_attr(not(feature = "gamepad"), allow(dead_code))]
    pub fn unload_engine() {
        #[cfg(feature = "speech")]
        crate::linux_speech::unload_engine();
    }
}

pub use desktop::{attach_input, attach_named, current_desktop_name, input_desktop_name};
pub use xbox_vk::{VkAttach, VkSession};
