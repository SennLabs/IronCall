//! Cookie jar (PLAN.md Phase 4): sent with matching requests and filled from every
//! `Set-Cookie`, redirects included. Persistence is the caller's job ([`CookieJar::to_json`]
//! and [`CookieJar::from_json`]).
//!
//! The jar has no public-suffix list, so it accepts `Domain=` attributes on suffixes such
//! as `co.uk`. That is acceptable for an API client talking to the user's own services.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cookie_store::{CookieDomain, CookieExpiration, CookieStore, RawCookie};
use http::header::SET_COOKIE;
use http::{HeaderMap, HeaderValue, Uri};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

/// A cookie as shown and edited in the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CookieInfo {
    pub domain: String,
    pub path: String,
    pub name: String,
    pub value: String,
    /// `None` for session cookies.
    pub expires: Option<SystemTime>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<SameSite>,
    /// Matches only `domain` itself (the cookie had no `Domain` attribute).
    pub host_only: bool,
}

/// A thread-safe cookie store. Writes are serialised by its lock, so parallel sends
/// never lose each other's cookies.
#[derive(Debug, Default)]
pub struct CookieJar {
    store: Mutex<CookieStore>,
}

impl CookieJar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Loads a jar written by [`CookieJar::to_json`].
    pub fn from_json(json: &str) -> Result<Self, String> {
        let store = cookie_store::serde::json::load_all(json.as_bytes()).map_err(|e| e.to_string())?;
        Ok(Self {
            store: Mutex::new(store),
        })
    }

    /// Serialises every unexpired cookie, session cookies included (they last until the
    /// user clears them, like a long-running browser session).
    pub fn to_json(&self) -> String {
        let store = self.lock();
        let mut out = Vec::new();
        let unexpired = CookieStore::from_cookies(
            store
                .iter_unexpired()
                .map(|c| Ok::<_, std::convert::Infallible>(c.clone())),
            false,
        )
        .expect("infallible");
        cookie_store::serde::json::save_incl_expired_and_nonpersistent(&unexpired, &mut out)
            .expect("writing JSON to memory cannot fail");
        String::from_utf8(out).expect("JSON is UTF-8")
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CookieStore> {
        self.store.lock().expect("cookie jar lock poisoned")
    }

    /// The `Cookie` header for a request to `url`, if any cookies match.
    pub fn header_for(&self, url: &Uri) -> Option<HeaderValue> {
        let url = to_url(url)?;
        let store = self.lock();
        let pairs: Vec<String> = store
            .get_request_values(&url)
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        if pairs.is_empty() {
            return None;
        }
        let mut value = HeaderValue::from_str(&pairs.join("; ")).ok()?;
        value.set_sensitive(true);
        Some(value)
    }

    /// Stores every `Set-Cookie` in a response to `url`.
    pub fn store_response(&self, url: &Uri, headers: &HeaderMap) {
        let Some(url) = to_url(url) else { return };
        let cookies: Vec<RawCookie<'static>> = headers
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|v| RawCookie::parse(v.to_string()).ok())
            .collect();
        if !cookies.is_empty() {
            self.lock().store_response_cookies(cookies.into_iter(), &url);
        }
    }

    pub fn list(&self) -> Vec<CookieInfo> {
        let store = self.lock();
        let mut out: Vec<CookieInfo> = store
            .iter_unexpired()
            .filter_map(|c| {
                let (domain, host_only) = match &c.domain {
                    CookieDomain::HostOnly(d) => (d.clone(), true),
                    CookieDomain::Suffix(d) => (d.clone(), false),
                    CookieDomain::NotPresent | CookieDomain::Empty => return None,
                };
                Some(CookieInfo {
                    domain,
                    path: c.path.as_ref().to_string(),
                    name: c.name().to_string(),
                    value: c.value().to_string(),
                    expires: match &c.expires {
                        CookieExpiration::AtUtc(t) => {
                            Some(UNIX_EPOCH + Duration::from_secs(t.unix_timestamp().max(0) as u64))
                        }
                        CookieExpiration::SessionEnd => None,
                    },
                    secure: c.secure().unwrap_or(false),
                    http_only: c.http_only().unwrap_or(false),
                    same_site: c.same_site().map(|s| match s {
                        cookie::SameSite::Strict => SameSite::Strict,
                        cookie::SameSite::Lax => SameSite::Lax,
                        cookie::SameSite::None => SameSite::None,
                    }),
                    host_only,
                })
            })
            .collect();
        out.sort_by(|a, b| (&a.domain, &a.path, &a.name).cmp(&(&b.domain, &b.path, &b.name)));
        out
    }

    /// Adds or replaces a cookie (matched by domain, path and name).
    pub fn set(&self, info: CookieInfo) -> Result<(), String> {
        let domain = info.domain.trim_start_matches('.').to_ascii_lowercase();
        if domain.is_empty() {
            return Err("a cookie needs a domain".into());
        }
        let path = if info.path.starts_with('/') {
            info.path.clone()
        } else {
            "/".to_string()
        };
        let mut raw = RawCookie::build((info.name.clone(), info.value.clone()))
            .path(path.clone())
            .secure(info.secure)
            .http_only(info.http_only);
        if !info.host_only {
            raw = raw.domain(domain.clone());
        }
        if let Some(same_site) = info.same_site {
            raw = raw.same_site(match same_site {
                SameSite::Strict => cookie::SameSite::Strict,
                SameSite::Lax => cookie::SameSite::Lax,
                SameSite::None => cookie::SameSite::None,
            });
        }
        if let Some(expires) = info.expires {
            let secs = expires
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let at = time::OffsetDateTime::from_unix_timestamp(secs as i64).map_err(|e| e.to_string())?;
            raw = raw.expires(at);
        }
        let scheme = if info.secure { "https" } else { "http" };
        let url = url::Url::parse(&format!("{scheme}://{domain}{path}")).map_err(|e| e.to_string())?;
        let mut store = self.lock();
        store.remove(&domain, &path, &info.name);
        store
            .insert_raw(&raw.build(), &url)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Removes one cookie. Returns whether it existed.
    pub fn delete(&self, domain: &str, path: &str, name: &str) -> bool {
        self.lock()
            .remove(domain.trim_start_matches('.'), path, name)
            .is_some()
    }

    pub fn clear(&self) {
        self.lock().clear();
    }
}

fn to_url(uri: &Uri) -> Option<url::Url> {
    url::Url::parse(&uri.to_string()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(s: &str) -> Uri {
        s.parse().unwrap()
    }

    fn set_cookie(values: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in values {
            h.append(SET_COOKIE, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn stores_and_sends_matching_cookies() {
        let jar = CookieJar::new();
        jar.store_response(
            &uri("https://api.test/login"),
            &set_cookie(&["session=abc; Path=/; HttpOnly", "pref=dark; Path=/settings"]),
        );
        assert_eq!(
            jar.header_for(&uri("https://api.test/me")).unwrap(),
            "session=abc"
        );
        let settings = jar.header_for(&uri("https://api.test/settings/x")).unwrap();
        assert!(settings.to_str().unwrap().contains("pref=dark"));
        assert!(jar.header_for(&uri("https://other.test/")).is_none());
    }

    #[test]
    fn json_round_trip_keeps_session_and_persistent_cookies() {
        let jar = CookieJar::new();
        jar.store_response(
            &uri("https://api.test/"),
            &set_cookie(&["s=1; Path=/", "p=2; Path=/; Max-Age=3600; Secure"]),
        );
        let restored = CookieJar::from_json(&jar.to_json()).unwrap();
        assert_eq!(restored.list(), jar.list());
        assert_eq!(restored.list().len(), 2);
    }

    #[test]
    fn set_delete_and_clear() {
        let jar = CookieJar::new();
        let cookie = CookieInfo {
            domain: "api.test".into(),
            path: "/".into(),
            name: "token".into(),
            value: "v1".into(),
            expires: None,
            secure: false,
            http_only: false,
            same_site: Some(SameSite::Lax),
            host_only: true,
        };
        jar.set(cookie.clone()).unwrap();
        jar.set(CookieInfo {
            value: "v2".into(),
            ..cookie.clone()
        })
        .unwrap();
        let listed = jar.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].value, "v2");
        assert_eq!(listed[0].same_site, Some(SameSite::Lax));
        assert!(jar.delete("api.test", "/", "token"));
        assert!(jar.list().is_empty());
        jar.set(cookie).unwrap();
        jar.clear();
        assert!(jar.list().is_empty());
    }
}
