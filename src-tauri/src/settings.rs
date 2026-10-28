//! General app settings (PLAN.md Phase 8): request defaults, appearance and extra
//! sensitive headers, in `<app data>/settings.json` next to the network and history
//! settings, which keep their own commands.

use ironcall_core::model::HttpVersion;
use ironcall_core::wire::WireDefaults;
use ironcall_store::settings::{RequestDefaults, Theme, UiSettings};
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::AppState;

/// Bounds for values typed in the settings UI.
const MAX_TIMEOUT_MS: u64 = 60 * 60 * 1000;
const MAX_REDIRECTS: u32 = 50;
const FONT_SIZES: std::ops::RangeInclusive<u32> = 10..=24;
const ZOOMS: std::ops::RangeInclusive<u32> = 50..=200;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ThemeChoice {
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RequestDefaultsInfo {
    /// 0 means no timeout.
    #[specta(type = specta_typescript::Number)]
    pub timeout_ms: u64,
    pub follow_redirects: bool,
    pub max_redirects: u32,
    pub http_version: HttpVersion,
    /// Always open a new connection, so timings are measured cold.
    pub fresh_connection: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceInfo {
    pub theme: ThemeChoice,
    /// In px.
    pub font_size: u32,
    /// In percent.
    pub zoom: u32,
}

/// Settings shown in the Settings dialog's General, Requests and Privacy sections.
#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GeneralSettings {
    pub requests: RequestDefaultsInfo,
    pub appearance: AppearanceInfo,
    /// Header names treated as sensitive in addition to the built-in list.
    pub sensitive_headers: Vec<String>,
}

fn to_info(requests: &RequestDefaults, ui: &UiSettings, sensitive: &[String]) -> GeneralSettings {
    GeneralSettings {
        requests: RequestDefaultsInfo {
            timeout_ms: requests.timeout_ms,
            follow_redirects: requests.follow_redirects,
            max_redirects: requests.max_redirects,
            http_version: requests.http_version,
            fresh_connection: requests.fresh_connection,
        },
        appearance: AppearanceInfo {
            theme: match ui.theme {
                Theme::System => ThemeChoice::System,
                Theme::Light => ThemeChoice::Light,
                Theme::Dark => ThemeChoice::Dark,
            },
            font_size: ui.font_size,
            zoom: ui.zoom,
        },
        sensitive_headers: sensitive.to_vec(),
    }
}

/// Cleans a header list: trimmed, valid header-name tokens, no duplicates (case-insensitive).
fn clean_headers(list: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for raw in list {
        let name = raw.trim();
        if name.is_empty() {
            continue;
        }
        if http::HeaderName::from_bytes(name.as_bytes()).is_err() {
            return Err(format!("`{name}` isn't a valid header name"));
        }
        if !out.iter().any(|h| h.eq_ignore_ascii_case(name)) {
            out.push(name.to_string());
        }
    }
    Ok(out)
}

impl crate::network::Network {
    pub(crate) fn general_settings(&self) -> GeneralSettings {
        let s = self.settings.load();
        to_info(&s.requests, &s.ui, &s.sensitive_headers)
    }

    /// User-configured sensitive headers, for redaction.
    pub(crate) fn sensitive_headers(&self) -> Vec<String> {
        self.settings.load().sensitive_headers
    }

    /// Defaults for requests that don't set these themselves.
    pub(crate) fn wire_defaults(&self) -> WireDefaults {
        let r = self.settings.load().requests;
        WireDefaults {
            timeout_ms: (r.timeout_ms > 0).then_some(r.timeout_ms),
            follow_redirects: r.follow_redirects,
            max_redirects: r.max_redirects,
            http_version: r.http_version,
            fresh_connection: r.fresh_connection,
            ..WireDefaults::default()
        }
    }

    pub(crate) fn save_general_settings(&self, g: &GeneralSettings) -> Result<GeneralSettings, String> {
        let r = &g.requests;
        if r.timeout_ms > MAX_TIMEOUT_MS {
            return Err("the timeout can be at most one hour".into());
        }
        if r.max_redirects > MAX_REDIRECTS {
            return Err(format!("at most {MAX_REDIRECTS} redirects"));
        }
        let a = &g.appearance;
        if !FONT_SIZES.contains(&a.font_size) {
            return Err(format!(
                "the font size must be {}–{} px",
                FONT_SIZES.start(),
                FONT_SIZES.end()
            ));
        }
        if !ZOOMS.contains(&a.zoom) {
            return Err(format!("zoom must be {}–{}%", ZOOMS.start(), ZOOMS.end()));
        }
        let mut settings = self.settings.load();
        settings.requests = RequestDefaults {
            timeout_ms: r.timeout_ms,
            follow_redirects: r.follow_redirects,
            max_redirects: r.max_redirects,
            http_version: r.http_version,
            fresh_connection: r.fresh_connection,
        };
        settings.ui = UiSettings {
            theme: match a.theme {
                ThemeChoice::System => Theme::System,
                ThemeChoice::Light => Theme::Light,
                ThemeChoice::Dark => Theme::Dark,
            },
            font_size: a.font_size,
            zoom: a.zoom,
        };
        settings.sensitive_headers = clean_headers(&g.sensitive_headers)?;
        self.settings.save(&settings).map_err(|e| e.to_string())?;
        Ok(to_info(
            &settings.requests,
            &settings.ui,
            &settings.sensitive_headers,
        ))
    }
}

#[tauri::command]
#[specta::specta]
pub fn settings_get(state: tauri::State<'_, AppState>) -> GeneralSettings {
    state.network.general_settings()
}

/// Validates and saves; returns what was stored (header names cleaned).
#[tauri::command]
#[specta::specta]
pub fn settings_set(
    state: tauri::State<'_, AppState>,
    settings: GeneralSettings,
) -> Result<GeneralSettings, String> {
    state.network.save_general_settings(&settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::Network;

    #[test]
    fn defaults_validation_and_header_cleaning() {
        let tmp = tempfile::tempdir().unwrap();
        let net = Network::new(tmp.path());
        let mut g = net.general_settings();
        assert_eq!(g.requests.timeout_ms, 30_000);
        assert_eq!(g.appearance.zoom, 100);
        assert_eq!(net.wire_defaults().timeout_ms, Some(30_000));

        g.requests.timeout_ms = 0;
        g.requests.http_version = HttpVersion::Http1;
        g.requests.fresh_connection = true;
        g.sensitive_headers = vec![
            " X-Session ".into(),
            "x-session".into(),
            "".into(),
            "X-Csrf".into(),
        ];
        let saved = net.save_general_settings(&g).unwrap();
        assert_eq!(saved.sensitive_headers, ["X-Session", "X-Csrf"]);
        assert_eq!(net.sensitive_headers(), ["X-Session", "X-Csrf"]);
        let d = net.wire_defaults();
        assert_eq!(d.timeout_ms, None, "0 means no timeout");
        assert_eq!(d.http_version, HttpVersion::Http1);
        assert!(d.fresh_connection);
        assert!(d.verify_tls, "TLS verification has no global off switch");

        for bad in [
            GeneralSettings {
                sensitive_headers: vec!["bad header".into()],
                ..saved.clone()
            },
            GeneralSettings {
                appearance: AppearanceInfo {
                    zoom: 500,
                    ..saved.appearance.clone()
                },
                ..saved.clone()
            },
            GeneralSettings {
                requests: RequestDefaultsInfo {
                    max_redirects: 99,
                    ..saved.requests.clone()
                },
                ..saved.clone()
            },
        ] {
            assert!(net.save_general_settings(&bad).is_err());
        }
        assert_eq!(net.general_settings(), saved, "rejected saves change nothing");
    }
}
