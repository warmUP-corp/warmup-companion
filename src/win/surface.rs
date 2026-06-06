//! The input Surface (CONTEXT.md: Userland vs Winlogon).
//!
//! One place to answer "which surface are we on?", so the decision stops being
//! re-derived from a raw desktop-name string at every call site. Probing stays
//! in [`super::desktop`]; this module owns the classification. Prediction is
//! Userland-only; the secure attach/render paths key off Winlogon.

use super::desktop;

/// Which input desktop is in play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// Default interactive desktop (`winsta0\default`).
    Userland,
    /// Secure desktop (`winsta0\winlogon`) — sign-in, lock, UAC.
    Winlogon,
}

impl Surface {
    pub fn is_winlogon(self) -> bool {
        matches!(self, Surface::Winlogon)
    }
    pub fn is_userland(self) -> bool {
        matches!(self, Surface::Userland)
    }
}

/// Classify a desktop name. The secure desktop is named "Winlogon"; every other
/// desktop (default, screen-saver, …) is treated as Userland.
pub fn classify(desktop_name: &str) -> Surface {
    if desktop_name.eq_ignore_ascii_case("winlogon") {
        Surface::Winlogon
    } else {
        Surface::Userland
    }
}

/// Surface of the current *input* desktop (the one receiving user input).
/// `None` when the probe fails — callers keep their own conservative fallback.
pub fn input() -> Option<Surface> {
    desktop::input_desktop_name().ok().map(|n| classify(&n))
}

/// Surface of the desktop *this thread* is attached to. `None` when unknown.
pub fn thread() -> Option<Surface> {
    desktop::current_desktop_name().map(|n| classify(&n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winlogon_name_is_the_secure_surface() {
        assert_eq!(classify("Winlogon"), Surface::Winlogon);
        assert_eq!(classify("winlogon"), Surface::Winlogon); // case-insensitive
        assert!(classify("WINLOGON").is_winlogon());
    }

    #[test]
    fn default_and_other_desktops_are_userland() {
        assert_eq!(classify("Default"), Surface::Userland);
        assert_eq!(classify("Screen-saver"), Surface::Userland);
        assert_eq!(classify(""), Surface::Userland);
        assert!(classify("Default").is_userland());
    }
}
