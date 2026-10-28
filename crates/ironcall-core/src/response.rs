//! A finished response, as seen by assertions and extraction (PLAN.md Phase 7).

use std::cell::OnceCell;

/// Bodies larger than this aren't loaded for assertions or extraction.
pub const MAX_CHECK_BODY: u64 = 32 * 1024 * 1024;

/// The decoded body text, or why it isn't available.
#[derive(Clone, Debug)]
pub enum BodyText {
    Text(String),
    /// Not loaded (too large, binary, or not needed); the reason is shown to the user.
    Unavailable(String),
}

/// What assertions and extraction can look at.
#[derive(Debug)]
pub struct ResponseData {
    pub status: u16,
    /// Final response headers, in order. Repeated names (e.g. `Set-Cookie`) stay separate.
    pub headers: Vec<(String, String)>,
    pub duration_ms: f64,
    /// Decoded body size in bytes.
    pub size: u64,
    pub body: BodyText,
    json: OnceCell<Result<serde_json::Value, String>>,
}

impl ResponseData {
    pub fn new(
        status: u16,
        headers: Vec<(String, String)>,
        duration_ms: f64,
        size: u64,
        body: BodyText,
    ) -> Self {
        Self {
            status,
            headers,
            duration_ms,
            size,
            body,
            json: OnceCell::new(),
        }
    }

    pub fn text(&self) -> Result<&str, String> {
        match &self.body {
            BodyText::Text(t) => Ok(t),
            BodyText::Unavailable(why) => Err(why.clone()),
        }
    }

    /// The body parsed as JSON, parsed once.
    pub fn json(&self) -> Result<&serde_json::Value, String> {
        self.json
            .get_or_init(|| {
                let text = self.text()?;
                serde_json::from_str(text).map_err(|e| format!("the body isn't valid JSON ({e})"))
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Values of a header (case-insensitive), in order.
    pub fn header_values<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.headers
            .iter()
            .filter(move |(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The value of cookie `name` set by this response's `Set-Cookie` headers (the last one wins).
    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.header_values("set-cookie")
            .filter_map(|h| {
                let pair = h.split(';').next()?;
                let (n, v) = pair.split_once('=')?;
                (n.trim() == name).then(|| v.trim())
            })
            .last()
    }
}

/// Runs a JSONPath query (RFC 9535). One match is returned as itself, several as an array,
/// none as `None`.
pub fn json_path(value: &serde_json::Value, path: &str) -> Result<Option<serde_json::Value>, String> {
    let path = path.trim();
    if path.is_empty() || path == "$" {
        return Ok(Some(value.clone()));
    }
    let query =
        serde_json_path::JsonPath::parse(path).map_err(|e| format!("invalid JSONPath `{path}`: {e}"))?;
    let nodes = query.query(value).all();
    Ok(match nodes.len() {
        0 => None,
        1 => Some(nodes[0].clone()),
        _ => Some(serde_json::Value::Array(nodes.into_iter().cloned().collect())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resp(body: &str) -> ResponseData {
        ResponseData::new(
            200,
            vec![
                ("Set-Cookie".into(), "a=1; Path=/".into()),
                ("set-cookie".into(), "session=abc; HttpOnly".into()),
                ("X-Multi".into(), "one".into()),
                ("x-multi".into(), "two".into()),
            ],
            12.0,
            body.len() as u64,
            BodyText::Text(body.into()),
        )
    }

    #[test]
    fn json_path_single_many_none_and_root() {
        let v = json!({"items": [{"id": 1}, {"id": 2}], "name": "x"});
        assert_eq!(json_path(&v, "$.name").unwrap(), Some(json!("x")));
        assert_eq!(json_path(&v, "$.items[*].id").unwrap(), Some(json!([1, 2])));
        assert_eq!(json_path(&v, "$.missing").unwrap(), None);
        assert_eq!(json_path(&v, "$").unwrap(), Some(v.clone()));
        assert_eq!(json_path(&v, "").unwrap(), Some(v.clone()));
        assert!(json_path(&v, "$[").unwrap_err().contains("invalid JSONPath"));
    }

    #[test]
    fn json_errors_and_unavailable_bodies() {
        assert!(resp("not json").json().unwrap_err().contains("isn't valid JSON"));
        let r = ResponseData::new(200, vec![], 0.0, 0, BodyText::Unavailable("too large".into()));
        assert_eq!(r.json().unwrap_err(), "too large");
    }

    #[test]
    fn headers_and_cookies() {
        let r = resp("{}");
        assert_eq!(r.header_values("X-MULTI").collect::<Vec<_>>(), ["one", "two"]);
        assert_eq!(r.cookie("session"), Some("abc"));
        assert_eq!(r.cookie("a"), Some("1"));
        assert_eq!(r.cookie("nope"), None);
    }
}
