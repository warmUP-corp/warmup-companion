//! Analyzed executable symbols from Ghidra (address -> inferred name).
#![allow(dead_code)]

// Functions
pub const FN_ATTACH_NAMED_DESKTOP: &str = "warmup_attach_named_desktop"; // 0041eac0
pub const FN_ATTACH_INPUT_DESKTOP: &str = "warmup_attach_input_desktop"; // 0041ec70
pub const FN_FOREGROUND_TIMER: &str = "warmup_foreground_timer_proc"; // 0044eb60
pub const FN_PROCESS_CONTROLLER_INPUT: &str = "warmup_process_controller_input"; // 004244f0
pub const FN_APPLY_MASK_SLOT_ACTION: &str = "warmup_apply_mask_slot_action"; // 00423510
pub const FN_EXECUTE_QUEUED_ACTION: &str = "warmup_execute_queued_action"; // 00428e00
pub const FN_CREATE_XBOX_VK_WINDOW: &str = "warmup_create_xbox_vk_window"; // 004672b0
pub const FN_XBOX_VK_THREAD_ENTRY: &str = "warmup_xbox_vk_thread_entry"; // 00467690
pub const FN_SPIRAL_VK_THREAD_ENTRY: &str = "warmup_spiral_vk_thread_entry"; // 00457560
pub const FN_CREATE_SPIRAL_VK_WINDOW: &str = "warmup_create_spiral_vk_window"; // 00456f90
pub const FN_INIT_APPLICATION: &str = "warmup_init_application"; // 00426080
pub const FN_PARSE_COMMAND_LINE: &str = "warmup_parse_command_line"; // 004199e0
pub const FN_ON_CONTROLLER_PRESS: &str = "warmup_on_controller_press"; // 00423120
pub const FN_ON_CONTROLLER_RELEASE: &str = "warmup_on_controller_release"; // 00423380
pub const FN_LAYOUT_VK_ON_MONITOR: &str = "warmup_layout_vk_on_monitor"; // 00467190

// Globals
pub const G_BOOT_SERVICE_MODE: &str = "g_boot_service_mode"; // DAT_004a6741
pub const G_FULLSCREEN_FG_FLAG: &str = "g_fullscreen_foreground_flag"; // DAT_004a6430
pub const G_APP_FEATURE_FLAGS: &str = "g_app_feature_flags"; // DAT_004a6684
pub const G_VK_OPEN_LATCH: &str = "g_vk_window_open_latch"; // DAT_004a74d0
