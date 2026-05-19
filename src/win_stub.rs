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
}

pub mod xbox_vk {
    pub struct VkSession;

    pub enum VkAttach {
        Current,
        Input,
    }

    impl VkSession {
        pub fn open(_attach: VkAttach) -> Result<Self, String> {
            Err("real on-screen keyboard requires Windows".into())
        }
        pub fn describe(&self) -> &'static str {
            "n/a"
        }
        pub fn close(self) {}
    }
}

pub use desktop::{attach_input, attach_named, current_desktop_name};
pub use xbox_vk::{VkAttach, VkSession};
