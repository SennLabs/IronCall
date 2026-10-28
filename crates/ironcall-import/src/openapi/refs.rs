//! Local `$ref` resolution and warning collection.

use std::cell::RefCell;
use std::collections::HashSet;

use serde_json::Value;

use super::Warning;

/// Longest `$ref` chain followed before giving up (cycles are caught earlier).
const MAX_CHAIN: usize = 32;

/// The parsed spec plus the warnings found while reading it.
pub(super) struct Doc<'a> {
    pub root: &'a Value,
    warnings: RefCell<Vec<Warning>>,
    /// Refs already warned about, so a broken ref used 500 times warns once.
    warned: RefCell<HashSet<String>>,
}

impl<'a> Doc<'a> {
    pub fn new(root: &'a Value) -> Self {
        Self {
            root,
            warnings: RefCell::new(Vec::new()),
            warned: RefCell::new(HashSet::new()),
        }
    }

    pub fn warn(&self, message: impl Into<String>, location: Option<&str>) {
        self.warnings.borrow_mut().push(Warning {
            message: message.into(),
            location: location.map(str::to_string),
        });
    }

    fn warn_once(&self, key: &str, message: String) {
        if self.warned.borrow_mut().insert(key.to_string()) {
            self.warn(message, Some(key));
        }
    }

    pub fn take_warnings(&self) -> Vec<Warning> {
        std::mem::take(&mut self.warnings.borrow_mut())
    }

    /// Follows `$ref`s (local ones only) until a non-ref value. `None` for external,
    /// broken or cyclic refs, each warned about once.
    pub fn deref(&self, value: &'a Value) -> Option<&'a Value> {
        let mut current = value;
        let mut seen: Vec<&str> = Vec::new();
        while let Some(target) = current.get("$ref").and_then(Value::as_str) {
            if seen.contains(&target) || seen.len() >= MAX_CHAIN {
                self.warn_once(target, format!("`$ref` cycle through `{target}`; ignored"));
                return None;
            }
            seen.push(target);
            let Some(pointer) = target.strip_prefix('#') else {
                self.warn_once(
                    target,
                    format!("external `$ref` to `{target}` isn't followed; import that file separately"),
                );
                return None;
            };
            match self.root.pointer(&unescape_pointer(pointer)) {
                Some(next) => current = next,
                None => {
                    self.warn_once(target, format!("`$ref` to `{target}` doesn't point to anything"));
                    return None;
                }
            }
        }
        Some(current)
    }

    /// `deref` then `get(key)` then `deref` again.
    pub fn get(&self, value: &'a Value, key: &str) -> Option<&'a Value> {
        self.deref(value)?.get(key).and_then(|v| self.deref(v))
    }
}

/// JSON pointers in refs may be percent-encoded (`#/paths/~1pets%7Bid%7D`).
fn unescape_pointer(pointer: &str) -> String {
    let bytes = pointer.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Escapes a path segment for a JSON pointer location (`/pets/{id}` → `~1pets~1{id}`).
pub(super) fn pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn follows_chains_and_stops_cycles_and_external_refs() {
        let root = json!({
            "a": {"$ref": "#/b"},
            "b": {"$ref": "#/c"},
            "c": {"type": "string"},
            "x": {"$ref": "#/y"},
            "y": {"$ref": "#/x"},
            "ext": {"$ref": "other.yaml#/Pet"},
            "broken": {"$ref": "#/nope"}
        });
        let doc = Doc::new(&root);
        assert_eq!(doc.deref(&root["a"]), Some(&json!({"type": "string"})));
        assert_eq!(doc.deref(&root["x"]), None);
        assert_eq!(doc.deref(&root["ext"]), None);
        assert_eq!(doc.deref(&root["broken"]), None);
        // Each problem warned about once, even if hit again.
        assert_eq!(doc.deref(&root["x"]), None);
        assert_eq!(doc.take_warnings().len(), 3);
    }

    #[test]
    fn pointer_escapes() {
        let root = json!({"paths": {"/pets/{id}": {"get": {"ok": true}}}});
        let doc = Doc::new(&root);
        let r = json!({"$ref": "#/paths/~1pets~1%7Bid%7D/get"});
        assert_eq!(doc.deref(&r), Some(&json!({"ok": true})));
        assert_eq!(pointer_segment("/a~b"), "~1a~0b");
    }
}
