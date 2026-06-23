//! Identyfikatory presetów motywu (`src/embed/theme-presets.json`).

use std::sync::OnceLock;

const THEME_PRESETS_JSON: &str = include_str!("embed/theme-presets.json");

static IDS: OnceLock<Vec<String>> = OnceLock::new();

pub fn theme_preset_ids() -> &'static [String] {
    IDS.get_or_init(|| {
        let root: serde_json::Value =
            serde_json::from_str(THEME_PRESETS_JSON).unwrap_or(serde_json::Value::Null);
        root.get("presets")
            .and_then(|p| p.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        item.get("id")
                            .and_then(|id| id.as_str())
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
    .as_slice()
}

pub fn is_allowed_theme_preset(id: &str) -> bool {
    theme_preset_ids().iter().any(|p| p == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_lists_core_presets() {
        let ids = theme_preset_ids();
        assert!(ids.len() >= 10);
        assert!(ids.iter().any(|id| id == "slavia"));
        assert!(is_allowed_theme_preset("pink"));
        assert!(!is_allowed_theme_preset("not-a-preset"));
    }
}
