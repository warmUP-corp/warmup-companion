//! Golden-fixture loader for cursor/scroll math parity (#346).
//!
//! Mirror of `warmUp/apps/desktop/src-tauri/src/gamepad/golden.rs` — the two
//! processes share the wire contract by checking in an identical fixture format
//! and loader (ADR 0002 / `docs/companion-ipc-protocol.md`), not a shared crate.
//! No parity assertions yet — those land with the injection slice (#349).

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

/// Tuning fields fed into the cursor/scroll math for a single case.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GoldenConfig {
    pub deadzone: f32,
    pub sensitivity: f32,
    pub acceleration_exp: f32,
    pub scroll_sensitivity: f32,
}

/// Inputs to the math under test: stick deflection, frame delta, and tuning.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GoldenInput {
    pub stick_x: f32,
    pub stick_y: f32,
    /// Frame delta in seconds.
    pub dt: f32,
    pub config: GoldenConfig,
}

/// Reference output `(dx, dy)`. `None` until #349 populates it from the canonical impl.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GoldenOutput {
    pub dx: f64,
    pub dy: f64,
}

/// One parity vector.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct GoldenCase {
    pub name: String,
    pub input: GoldenInput,
    pub expected: Option<GoldenOutput>,
}

/// The checked-in golden fixture, shared verbatim by both repos.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GoldenFixture {
    pub version: u32,
    /// Maps desktop config field names to the companion's internal names
    /// (`sensitivity->cursor_speed`, etc.).
    pub config_field_mapping: BTreeMap<String, String>,
    pub cases: Vec<GoldenCase>,
}

impl GoldenFixture {
    /// Parse a fixture from its JSON text.
    pub fn load_from_str(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| e.to_string())
    }

    /// Read and parse a fixture file from disk.
    pub fn load_from_path(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Self::load_from_str(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn fixture_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cursor-scroll-golden.json")
    }

    #[test]
    fn loads_versioned_fixture_from_disk() {
        let fx = GoldenFixture::load_from_path(&fixture_path()).expect("fixture should load");
        assert_eq!(fx.version, 1);
    }

    #[test]
    fn exposes_the_four_config_field_mappings() {
        let fx = GoldenFixture::load_from_path(&fixture_path()).expect("fixture should load");
        assert_eq!(
            fx.config_field_mapping
                .get("sensitivity")
                .map(String::as_str),
            Some("cursor_speed")
        );
        assert_eq!(
            fx.config_field_mapping
                .get("accelerationExp")
                .map(String::as_str),
            Some("cursor_accel")
        );
        assert_eq!(
            fx.config_field_mapping.get("deadzone").map(String::as_str),
            Some("cursor_deadzone")
        );
        assert_eq!(
            fx.config_field_mapping
                .get("scrollSensitivity")
                .map(String::as_str),
            Some("scroll_speed")
        );
    }

    #[test]
    fn parses_cases_with_input_and_optional_expected() {
        let fx = GoldenFixture::load_from_path(&fixture_path()).expect("fixture should load");
        assert!(
            !fx.cases.is_empty(),
            "fixture should carry at least one case"
        );
        // No parity values are asserted in this slice (#346) — expected is unpopulated until #349.
        assert!(
            fx.cases.iter().all(|c| c.expected.is_none()),
            "expected outputs are populated in #349, not here"
        );
        let centered = fx
            .cases
            .iter()
            .find(|c| c.input.stick_x == 0.0 && c.input.stick_y == 0.0)
            .expect("a centered-stick case should exist");
        assert!(centered.input.dt > 0.0);
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(GoldenFixture::load_from_str("{ not json").is_err());
    }
}
