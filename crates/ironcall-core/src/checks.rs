//! Assertions and extraction on a finished response, shared by single sends and the
//! collection runner (PLAN.md Phase 7). Cancelled responses must never get here.

use crate::assertions::{self, AssertionResult};
use crate::extract::{self, Extracted};
use crate::model::{AssertTarget, RequestFile};
use crate::response::ResponseData;
use crate::vars::Resolver;

/// Results of checking one response.
#[derive(Clone, Debug, Default)]
pub struct Checked {
    pub assertions: Vec<AssertionResult>,
    pub extracted: Vec<Extracted>,
}

impl Checked {
    /// All enabled assertions passed and every extraction succeeded.
    pub fn passed(&self) -> bool {
        self.assertions.iter().all(|a| a.passed) && self.extracted.iter().all(|e| e.value.is_ok())
    }
}

/// Whether the response body must be loaded to check `request`.
pub fn needs_body(request: &RequestFile) -> bool {
    extract::needs_body(&request.extract)
        || request
            .assert
            .iter()
            .any(|a| a.enabled && matches!(a.target, AssertTarget::Body | AssertTarget::BodyText))
}

/// Evaluates `request`'s assertions and extractions. `resolver` resolves `{{variables}}`
/// in expected values; it should be the one the request was sent with.
pub fn check_response(
    request: &RequestFile,
    resolver: &Resolver,
    response: &ResponseData,
    sensitive_headers: &[String],
) -> Checked {
    let resolved = assertions::resolve(&request.assert, resolver);
    let mut secrets = resolved.secrets;
    // A sensitive value extracted from this response must not show up as an actual value.
    let extracted = extract::run(&request.extract, response, sensitive_headers);
    secrets.extend(
        extracted
            .iter()
            .filter(|e| e.sensitive)
            .filter_map(|e| e.value.as_ref().ok().cloned()),
    );
    Checked {
        assertions: assertions::evaluate(
            &request.assert,
            &resolved.value,
            response,
            &secrets,
            sensitive_headers,
        ),
        extracted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AssertOp, Assertion, ExtractSource, Extraction};
    use crate::response::BodyText;
    use serde_json::json;

    fn request() -> RequestFile {
        let mut r = RequestFile::new("r");
        r.assert.push(Assertion {
            target: AssertTarget::BodyText,
            path: None,
            name: None,
            op: AssertOp::Contains,
            value: Some(json!("nope")),
            enabled: true,
            extra: Default::default(),
        });
        r.extract.push(Extraction {
            var: "token".into(),
            from: ExtractSource::Body,
            path: Some("$.t".into()),
            name: None,
            pattern: None,
            sensitive: false,
            extra: Default::default(),
        });
        r
    }

    #[test]
    fn sensitive_extractions_are_masked_in_assertion_output() {
        let body = r#"{"t":"tok-very-secret"}"#;
        let resp = ResponseData::new(200, vec![], 1.0, body.len() as u64, BodyText::Text(body.into()));
        let checked = check_response(&request(), &Resolver::default(), &resp, &[]);
        assert!(!checked.passed());
        assert!(checked.extracted[0].sensitive, "`token` is a sensitive name");
        let shown = checked.assertions[0].actual.clone().unwrap();
        assert!(!shown.contains("tok-very-secret"), "{shown}");
    }

    #[test]
    fn needs_body_only_when_a_check_reads_it() {
        let mut r = RequestFile::new("r");
        assert!(!needs_body(&r));
        r.assert.push(Assertion {
            target: AssertTarget::Status,
            path: None,
            name: None,
            op: AssertOp::Eq,
            value: Some(json!(200)),
            enabled: true,
            extra: Default::default(),
        });
        assert!(!needs_body(&r));
        let mut disabled = request().assert.remove(0);
        disabled.enabled = false;
        r.assert.push(disabled);
        assert!(!needs_body(&r));
        assert!(needs_body(&request()));
    }
}
