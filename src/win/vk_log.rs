//! VK UI logging (stderr + service.log when `WARMUP_VK_SERVICE=1`).

pub fn log(msg: &str) {
    eprintln!("> vk ui: {msg}");
    if crate::config::service_mode() {
        crate::install::log_line(&format!("vk ui: {msg}"));
    }
}
