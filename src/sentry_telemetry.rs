//! Opt-in crash telemetry.
//!
//! Disabled unless `WARMUP_SENTRY_DSN` is set. This keeps OSS builds transparent:
//! no DSN means no client, no network transport, and no panic reporting.

use std::borrow::Cow;

pub fn init() -> Option<sentry::ClientInitGuard> {
    let dsn = std::env::var("WARMUP_SENTRY_DSN").ok()?;
    if dsn.trim().is_empty() || std::env::var_os("WARMUP_SENTRY_DISABLED").is_some() {
        return None;
    }

    let release = std::env::var("WARMUP_SENTRY_RELEASE")
        .ok()
        .map(Cow::Owned)
        .or_else(|| {
            Some(Cow::Owned(format!(
                "warmup-companion@{}",
                env!("CARGO_PKG_VERSION")
            )))
        });
    let environment = std::env::var("WARMUP_SENTRY_ENV")
        .ok()
        .map(Cow::Owned)
        .or({
            Some(Cow::Borrowed(if cfg!(debug_assertions) {
                "development"
            } else {
                "production"
            }))
        });

    let guard = sentry::init((
        dsn,
        sentry::ClientOptions {
            release,
            environment,
            send_default_pii: false,
            server_name: None,
            attach_stacktrace: true,
            traces_sample_rate: 0.0,
            enable_logs: false,
            enable_metrics: false,
            ..Default::default()
        },
    ));

    sentry::configure_scope(|scope| {
        scope.set_tag("component", "warmup-companion");
        scope.set_tag("service_mode", crate::config::service_mode().to_string());
    });

    Some(guard)
}

pub fn capture_panic(info: &std::panic::PanicHookInfo<'_>, component: &'static str) {
    sentry::configure_scope(|scope| {
        scope.set_tag("component", component);
        scope.set_tag("service_mode", crate::config::service_mode().to_string());
    });
    sentry::integrations::panic::panic_handler(info);
}

#[cfg(windows)]
pub fn capture_native_crash(summary: String) {
    sentry::configure_scope(|scope| {
        scope.set_tag("component", "service-worker");
        scope.set_tag("crash_kind", "win32_seh");
        scope.set_tag("service_mode", crate::config::service_mode().to_string());
    });
    sentry::capture_message(&summary, sentry::Level::Fatal);
    if let Some(client) = sentry::Hub::current().client() {
        client.flush(None);
    }
}
