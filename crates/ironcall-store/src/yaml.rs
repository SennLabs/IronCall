//! Reading and writing workspace YAML files (PLAN.md §4.4, ADR 0002).
//!
//! Output is canonical: fixed key order, every string quoted, multiline strings as `|`
//! blocks, table rows as one-line flow maps, LF line endings and a trailing newline.
//! Parsing a canonical file and writing it again gives byte-identical output.
//!
//! Known limits (documented in ADR 0002): YAML comments are dropped; hand-written
//! formatting is normalised on the first save; unknown keys are written after the known
//! ones, and unquoted scalars inside unknown keys keep their YAML type (`no` → `false`).

use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug, thiserror::Error)]
pub enum YamlError {
    /// The file still contains git merge-conflict markers.
    #[error("unresolved merge conflict (marker at line {line})")]
    MergeConflict { line: usize },
    /// Invalid YAML, or YAML that doesn't match the file schema.
    #[error("line {line}, column {column}: {message}")]
    Invalid {
        line: u64,
        column: u64,
        message: String,
        /// Full message with a source snippet, for showing in the UI.
        detail: String,
    },
    /// Invalid YAML whose position the parser couldn't report.
    #[error("{message}")]
    InvalidNoLocation { message: String },
    #[error("cannot write YAML: {0}")]
    Serialize(String),
}

impl YamlError {
    /// 1-based line of the problem, if known.
    pub fn line(&self) -> Option<u64> {
        match self {
            Self::MergeConflict { line } => Some(*line as u64),
            Self::Invalid { line, .. } => Some(*line),
            _ => None,
        }
    }
}

/// Parses a workspace file.
pub fn from_str<T: DeserializeOwned>(text: &str) -> Result<T, YamlError> {
    if let Some(line) = merge_conflict_line(text) {
        return Err(YamlError::MergeConflict { line });
    }
    serde_saphyr::from_str(text).map_err(|err| {
        let detail = err.to_string();
        let message = first_message_line(&err.without_snippet().to_string());
        match err.location() {
            Some(loc) => YamlError::Invalid {
                line: loc.line(),
                column: loc.column(),
                message,
                detail,
            },
            None => YamlError::InvalidNoLocation { message },
        }
    })
}

/// Writes a workspace file in canonical form.
///
/// `T: DeserializeOwned + PartialEq` is needed to check the whitespace clean-up below.
pub fn to_string<T>(value: &T) -> Result<String, YamlError>
where
    T: Serialize + DeserializeOwned + PartialEq,
{
    let options = serde_saphyr::ser_options! {
        quote_all: true,
        // Only fields marked with `style::multiline` become blocks; plain strings stay quoted.
        prefer_block_scalars: false,
    };
    let mut out = serde_saphyr::to_string_with_options(value, options)
        .map_err(|e| YamlError::Serialize(e.to_string()))?;
    if out.contains('\r') {
        out = out.replace("\r\n", "\n");
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }

    // Empty lines inside `|` blocks are emitted as indentation-only lines. Editors and git
    // flag that trailing whitespace, so blank them out. A content line that really is only
    // spaces would change meaning, so keep the original output if the value differs.
    if out
        .lines()
        .any(|l| !l.is_empty() && l.trim_start_matches(' ').is_empty())
    {
        let cleaned = blank_whitespace_only_lines(&out);
        if serde_saphyr::from_str::<T>(&cleaned).is_ok_and(|v| v == *value) {
            out = cleaned;
        }
    }
    Ok(out)
}

fn blank_whitespace_only_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        if body.trim_start_matches(' ').is_empty() {
            out.push_str(&line[body.len()..]);
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Returns the 1-based line of the first git conflict marker, if any.
///
/// Checked before parsing because the YAML parser's error for these is confusing.
fn merge_conflict_line(text: &str) -> Option<usize> {
    text.lines().position(is_conflict_marker).map(|i| i + 1)
}

fn is_conflict_marker(line: &str) -> bool {
    let line = line.trim_end_matches('\r');
    line.starts_with("<<<<<<< ")
        || line == "<<<<<<<"
        || line.starts_with(">>>>>>> ")
        || line == ">>>>>>>"
        || line == "======="
        || line.starts_with("||||||| ")
}

/// Strips serde-saphyr's `error: line X column Y:` prefix (we report position separately).
fn first_message_line(rendered: &str) -> String {
    let first = rendered.lines().next().unwrap_or(rendered).trim();
    let first = first.strip_prefix("error:").unwrap_or(first).trim();
    match first.split_once(": ") {
        Some((prefix, rest)) if prefix.starts_with("line ") => rest.to_string(),
        _ => first.to_string(),
    }
}

/// Version of a `schema` value such as `ironcall/request@1`, if it is of `kind`.
pub fn schema_version(schema: &str, kind: &str) -> Option<u32> {
    let (name, version) = schema.split_once('@')?;
    (name.strip_prefix("ironcall/")? == kind).then_some(())?;
    version.parse().ok()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use ironcall_core::model::{
        Auth, Body, CollectionFile, EnvironmentFile, FolderFile, KeyValue, RequestFile, Variable, schema,
    };
    use proptest::prelude::*;
    use serde::Deserialize;

    use super::*;

    fn fixtures(sub: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/yaml")
            .join(sub)
    }

    fn fixture_files(sub: &str) -> Vec<PathBuf> {
        let mut files: Vec<_> = std::fs::read_dir(fixtures(sub))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "yaml"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "no fixtures in {sub}");
        files
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    /// Parses a file as the type named by its `schema` and writes it back.
    fn rewrite(text: &str) -> Result<String, YamlError> {
        #[derive(Deserialize)]
        struct Peek {
            schema: String,
        }
        let peek: Peek = from_str(text)?;
        match peek.schema.as_str() {
            schema::REQUEST => to_string(&from_str::<RequestFile>(text)?),
            schema::ENVIRONMENT => to_string(&from_str::<EnvironmentFile>(text)?),
            schema::FOLDER => to_string(&from_str::<FolderFile>(text)?),
            schema::COLLECTION => to_string(&from_str::<CollectionFile>(text)?),
            other => panic!("unknown schema {other}"),
        }
    }

    #[test]
    fn canonical_fixtures_are_byte_identical_after_round_trip() {
        for path in fixture_files("canonical") {
            let original = read(&path);
            let written = rewrite(&original).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            if written != original {
                panic!(
                    "{} changed on re-save.\n--- original\n{original}\n--- written\n{written}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn handwritten_fixtures_normalise_to_a_stable_canonical_form() {
        for path in fixture_files("handwritten") {
            let first = rewrite(&read(&path)).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let second = rewrite(&first).unwrap();
            assert_eq!(first, second, "{} is not stable after one save", path.display());
        }
    }

    #[test]
    fn plan_example_parses_to_expected_values() {
        let req: RequestFile = from_str(&read(&fixtures("handwritten/plan-request-example.yaml"))).unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.path_params[0].name, "orgId");
        assert!(req.auth.is_inherit());
        assert!(matches!(&req.body, Body::Json { content, .. } if content.starts_with("{\n  \"name\"")));
        assert_eq!(req.assert.len(), 3);
        assert_eq!(req.settings.timeout_ms, Some(30000));

        let canonical = read(&fixtures("canonical/request-full.yaml"));
        assert_eq!(
            rewrite(&read(&fixtures("handwritten/plan-request-example.yaml"))).unwrap(),
            canonical
        );
    }

    #[test]
    fn every_string_is_quoted() {
        let mut req: RequestFile = from_str(&read(&fixtures("canonical/request-minimal.yaml"))).unwrap();
        req.name = "no".into();
        req.headers = ["yes", "1.0", "0123", "null", "~", "2026-10-05", "true", ""]
            .iter()
            .map(|v| KeyValue::new("X", *v))
            .collect();
        let out = to_string(&req).unwrap();
        assert!(out.contains("name: 'no'\n"), "{out}");
        for v in [
            "'yes'",
            "'1.0'",
            "'0123'",
            "'null'",
            "'~'",
            "'2026-10-05'",
            "'true'",
            "''",
        ] {
            assert!(out.contains(&format!("value: {v}")), "{v} not quoted in:\n{out}");
        }
        let back: RequestFile = from_str(&out).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn multiline_strings_are_literal_blocks_and_single_lines_are_not() {
        let mut req: RequestFile = from_str(&read(&fixtures("canonical/request-minimal.yaml"))).unwrap();
        req.description = Some("one line".into());
        req.body = Body::Text {
            content: "a\nb\n".into(),
            extra: Default::default(),
        };
        let out = to_string(&req).unwrap();
        assert!(out.contains("description: 'one line'\n"), "{out}");
        assert!(out.contains("  content: |\n    a\n    b\n"), "{out}");
    }

    #[test]
    fn body_whitespace_survives_exactly() {
        for content in [
            "",
            "x",
            "x\n",
            "x\n\n\n",
            "  indented first\nx\n",
            "\ttab\n",
            "trailing space \n",
            "\n",
        ] {
            let mut req: RequestFile = from_str(&read(&fixtures("canonical/request-minimal.yaml"))).unwrap();
            req.body = Body::Text {
                content: content.into(),
                extra: Default::default(),
            };
            let back: RequestFile = from_str(&to_string(&req).unwrap()).unwrap();
            assert_eq!(back.body, req.body, "content {content:?}");
        }
    }

    #[test]
    fn blank_lines_in_blocks_have_no_trailing_whitespace() {
        let mut req: RequestFile = from_str(&read(&fixtures("canonical/request-minimal.yaml"))).unwrap();
        req.description = Some("para one\n\npara two\n".into());
        let out = to_string(&req).unwrap();
        assert!(out.lines().all(|l| !l.ends_with(' ')), "{out:?}");
        assert_eq!(from_str::<RequestFile>(&out).unwrap(), req);
    }

    #[test]
    fn whitespace_only_content_lines_are_kept() {
        let mut req: RequestFile = from_str(&read(&fixtures("canonical/request-minimal.yaml"))).unwrap();
        req.body = Body::Text {
            content: "a\n   \nb\n".into(),
            extra: Default::default(),
        };
        let out = to_string(&req).unwrap();
        assert_eq!(from_str::<RequestFile>(&out).unwrap(), req);
    }

    #[test]
    fn unknown_keys_are_preserved_after_known_keys() {
        let text = read(&fixtures("canonical/request-unknown-keys.yaml"));
        let req: RequestFile = from_str(&text).unwrap();
        assert!(req.extra.contains_key("futureTopLevel"));
        assert_eq!(req.headers[0].extra["futureRowKey"], "kept");
        assert!(matches!(&req.auth, Auth::Bearer { extra, .. } if extra["futureAuthKey"] == true));
        assert!(matches!(&req.body, Body::Json { extra, .. } if extra["futureBodyKey"] == "kept"));
        assert_eq!(req.settings.extra["futureSetting"], 2.5);

        // A key added by hand in the middle moves after the known keys on save.
        let moved = text.replace("method: 'GET'\n", "method: 'GET'\nmidKey: 'x'\n");
        let out = rewrite(&moved).unwrap();
        assert!(
            out.find("midKey").unwrap() > out.find("settings:").unwrap(),
            "{out}"
        );
    }

    #[test]
    fn unquoted_scalars_in_typed_fields_stay_text() {
        let req: RequestFile = from_str(&read(&fixtures("handwritten/unquoted-scalars.yaml"))).unwrap();
        assert_eq!(req.name, "no");
        let values: Vec<_> = req.params.iter().map(|p| p.value.as_str()).collect();
        assert_eq!(values, ["1.0", "yes", "2026-10-05", "0123"]);
        assert_eq!(req.vars["port"], Variable::Plain("8080".into()));
        assert_eq!(req.vars["debug"], Variable::Plain("true".into()));
        // Known limit: unquoted floats in variables are re-formatted (ADR 0002).
        assert_eq!(req.vars["ratio"], Variable::Plain("1.5".into()));
    }

    #[test]
    fn secret_variables_have_no_value_in_the_file() {
        let env: EnvironmentFile = from_str(&read(&fixtures("canonical/environment-dev.yaml"))).unwrap();
        assert!(env.vars["apiKey"].is_secret());
        assert!(!env.vars["baseUrl"].is_secret());
        let out = to_string(&env).unwrap();
        assert!(out.contains("  apiKey:\n    secret: true\n"), "{out}");
    }

    #[test]
    fn crlf_input_is_read_and_written_as_lf() {
        let text = read(&fixtures("canonical/request-full.yaml"));
        let crlf = text.replace('\n', "\r\n");
        let out = rewrite(&crlf).unwrap();
        assert!(!out.contains('\r'));
        assert_eq!(out, text);
    }

    #[test]
    fn output_ends_with_exactly_one_newline() {
        let out = rewrite(&read(&fixtures("canonical/request-minimal.yaml"))).unwrap();
        assert!(out.ends_with('\n') && !out.ends_with("\n\n"));
    }

    #[test]
    fn merge_conflict_markers_give_a_clear_error() {
        let err = from_str::<RequestFile>(&read(&fixtures("invalid/merge-conflict.yaml"))).unwrap_err();
        assert!(matches!(err, YamlError::MergeConflict { line: 3 }), "{err:?}");
        assert_eq!(err.to_string(), "unresolved merge conflict (marker at line 3)");
    }

    #[test]
    fn syntax_errors_report_the_line() {
        let err = from_str::<RequestFile>(&read(&fixtures("invalid/syntax-error.yaml"))).unwrap_err();
        let YamlError::Invalid { line, detail, .. } = &err else {
            panic!("{err:?}")
        };
        assert!((3..=5).contains(line), "line {line}: {err}");
        assert!(detail.contains("Broken"), "snippet missing: {detail}");
    }

    #[test]
    fn schema_errors_are_reported_not_ignored() {
        for (file, needle) in [
            ("missing-field.yaml", "method"),
            // Message is "invalid length" from the ULID parser; the position is what matters.
            ("bad-id.yaml", "line 2"),
            ("unknown-auth-type.yaml", "oauth2"),
        ] {
            let err = from_str::<RequestFile>(&read(&fixtures("invalid").join(file))).unwrap_err();
            assert!(
                err.to_string().to_lowercase().contains(needle),
                "{file}: expected {needle:?} in {err}"
            );
        }
    }

    #[test]
    fn schema_version_parses_kind_and_number() {
        assert_eq!(schema_version("ironcall/request@1", "request"), Some(1));
        assert_eq!(schema_version("ironcall/request@12", "request"), Some(12));
        assert_eq!(schema_version("ironcall/folder@1", "request"), None);
        assert_eq!(schema_version("other/request@1", "request"), None);
        assert_eq!(schema_version("ironcall/request", "request"), None);
    }

    fn arb_text() -> impl Strategy<Value = String> {
        // Mix of YAML-hostile characters, unicode and newlines.
        proptest::string::string_regex("[ -~\\t\\n\u{a0}-\u{2fff}🚀]{0,60}").unwrap()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn arbitrary_strings_round_trip(name in arb_text(), value in arb_text(), body in arb_text(), desc in arb_text()) {
            let mut req: RequestFile = from_str(&read(&fixtures("canonical/request-minimal.yaml"))).unwrap();
            req.name = name;
            req.description = Some(desc);
            req.headers = vec![KeyValue::new("X-Test", value)];
            req.body = Body::Text { content: body, extra: Default::default() };
            let written = to_string(&req).unwrap();
            let back: RequestFile = from_str(&written).unwrap();
            prop_assert_eq!(&back, &req);
            prop_assert_eq!(to_string(&back).unwrap(), written);
        }
    }
}
