pub mod desktop;
mod vk_log;
pub mod vk_ui;
pub mod xbox_vk;

pub use desktop::{attach_input, attach_named, current_desktop_name, sync_input_desktop};
pub use vk_ui::{is_vk_visible, request_repaint, tick_dpad_hold, VkAttach};
pub use xbox_vk::VkSession;
