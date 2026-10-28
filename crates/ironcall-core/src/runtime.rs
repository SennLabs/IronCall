//! The runtime variable scope: values set by `extract` during a session or a run
//! (PLAN.md §5, the highest-precedence layer). Never written to disk.

use indexmap::IndexMap;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use crate::extract::Extracted;
use crate::redact;
use crate::vars::{Def, Layer, Source};

#[derive(Clone, Debug)]
struct Value {
    value: SecretString,
    sensitive: bool,
}

/// Runtime variables, in the order they were first set.
#[derive(Clone, Debug, Default)]
pub struct RuntimeVars {
    vars: IndexMap<String, Value>,
}

/// A runtime variable for display: sensitive values masked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct RuntimeVarView {
    pub name: String,
    pub value: String,
    pub sensitive: bool,
}

impl RuntimeVars {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, name: impl Into<String>, value: SecretString, sensitive: bool) {
        self.vars.insert(name.into(), Value { value, sensitive });
    }

    /// Stores every successful extraction. Failed ones leave the previous value alone.
    pub fn apply(&mut self, extracted: &[Extracted]) {
        for e in extracted {
            if let Ok(v) = &e.value {
                self.set(e.var.clone(), v.clone(), e.sensitive);
            }
        }
    }

    /// Adds `other`'s values over this scope's.
    pub fn merge(&mut self, other: &RuntimeVars) {
        for (name, v) in &other.vars {
            self.vars.insert(name.clone(), v.clone());
        }
    }

    pub fn get(&self, name: &str) -> Option<&SecretString> {
        self.vars.get(name).map(|v| &v.value)
    }

    /// The value and whether it's sensitive.
    pub fn entry(&self, name: &str) -> Option<(&SecretString, bool)> {
        self.vars.get(name).map(|v| (&v.value, v.sensitive))
    }

    pub fn remove(&mut self, name: &str) {
        self.vars.shift_remove(name);
    }

    pub fn clear(&mut self) {
        self.vars.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }

    pub fn len(&self) -> usize {
        self.vars.len()
    }

    /// The resolver layer. Sensitive values become secrets, so they are masked in previews
    /// and redacted from history and logs like any other secret.
    pub fn layer(&self) -> Layer {
        let vars = self
            .vars
            .iter()
            .map(|(name, v)| {
                let def = if v.sensitive {
                    Def::Secret {
                        value: Some(v.value.clone()),
                        description: None,
                    }
                } else {
                    Def::Plain {
                        value: v.value.expose_secret().to_string(),
                        description: None,
                    }
                };
                (name.clone(), def)
            })
            .collect();
        Layer {
            source: Source::Runtime,
            vars,
        }
    }

    /// For display. `reveal` shows sensitive values too.
    pub fn views(&self, reveal: bool) -> Vec<RuntimeVarView> {
        self.vars
            .iter()
            .map(|(name, v)| RuntimeVarView {
                name: name.clone(),
                value: if v.sensitive && !reveal {
                    redact::MASK.to_string()
                } else {
                    v.value.expose_secret().to_string()
                },
                sensitive: v.sensitive,
            })
            .collect()
    }

    /// All sensitive values, for masking.
    pub fn secrets(&self) -> Vec<SecretString> {
        self.vars
            .values()
            .filter(|v| v.sensitive)
            .map(|v| v.value.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vars::Resolver;

    fn extracted(var: &str, value: Result<&str, &str>, sensitive: bool) -> Extracted {
        Extracted {
            var: var.into(),
            value: value
                .map(|v| SecretString::from(v.to_string()))
                .map_err(str::to_string),
            sensitive,
        }
    }

    #[test]
    fn apply_keeps_old_values_on_failure_and_overrides_on_success() {
        let mut rt = RuntimeVars::new();
        rt.apply(&[extracted("id", Ok("1"), false), extracted("tok", Ok("t"), true)]);
        rt.apply(&[
            extracted("id", Err("no match"), false),
            extracted("tok", Ok("t2"), true),
        ]);
        assert_eq!(rt.get("id").unwrap().expose_secret(), "1");
        assert_eq!(rt.get("tok").unwrap().expose_secret(), "t2");
        assert_eq!(rt.len(), 2);
    }

    #[test]
    fn layer_wins_over_request_and_treats_sensitive_values_as_secrets() {
        let mut rt = RuntimeVars::new();
        rt.set("id", SecretString::from("rt"), false);
        rt.set("tok", SecretString::from("secret-tok"), true);
        let r = Resolver::new(vec![rt.layer(), Layer::new(Source::Request).with("id", "req")]);
        assert_eq!(r.resolve_str("{{id}}").text, "rt");
        let tok = r.resolve_str("Bearer {{tok}}");
        assert_eq!(tok.text, "Bearer secret-tok");
        assert_eq!(tok.secrets.len(), 1, "sensitive values are reported as secrets");
        let visible = r.visible(false);
        let v = visible.iter().find(|v| v.name == "tok").unwrap();
        assert_eq!(v.value, None, "masked unless revealed");
    }

    #[test]
    fn views_merge_and_debug_hide_sensitive_values() {
        let mut a = RuntimeVars::new();
        a.set("x", SecretString::from("1"), false);
        let mut b = RuntimeVars::new();
        b.set("x", SecretString::from("2"), false);
        b.set("pw", SecretString::from("hunter2"), true);
        a.merge(&b);
        let views = a.views(false);
        assert_eq!(views[0].value, "2");
        assert_eq!(views[1].value, redact::MASK);
        assert_eq!(a.views(true)[1].value, "hunter2");
        assert!(!format!("{a:?}").contains("hunter2"));
        assert_eq!(a.secrets().len(), 1);
        a.remove("x");
        assert_eq!(a.len(), 1);
        a.clear();
        assert!(a.is_empty());
    }
}
