//! VK UI logging (stderr + service.log when `WARMUP_VK_SERVICE=1`).

pub fn log(msg: &str) {
    eprintln!("> vk ui: {msg}");
    if std::env::var_os("WARMUP_VK_SERVICE").is_some_and(|v| v != "0") {
        crate::install::log_line(&format!("vk ui: {msg}"));
    }
}
