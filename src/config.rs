//! Runtime config seam.
//!
//! The one place that reads `WARMUP_VK_SERVICE`. Every site that used to inspect
//! the env var inline now calls [`service_mode`], so "are we the boot/service
//! worker?" has a single definition. Feeds the `vk_gate` decision input.

/// `WARMUP_VK_SERVICE` is set (boot/service worker path). The ONE env read for
/// service mode.
pub fn service_mode() -> bool {
    std::env::var_os("WARMUP_VK_SERVICE").is_some_and(|v| v != "0")
}
