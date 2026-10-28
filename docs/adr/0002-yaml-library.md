# 0002 — YAML library and canonical file format

- **Status:** Accepted
- **Date:** 2026-10-05
- **Phase:** P0 (Spike B)

## Context
Workspace files are one YAML file per request, folder, environment and collection (PLAN.md §4). They must diff cleanly in git and survive hand edits and merges. Requirements from §4.4:
- every string quoted on write, and no implicit typing surprises (`no`, `1.0`, `0123`, `null`, `~`, dates stay strings)
- multiline strings (bodies, descriptions) written as `|` literal blocks
- stable key order, LF line endings, trailing newline
- re-saving an unchanged file gives **byte-identical** output
- unknown keys are preserved
- clear errors with line numbers, including for files with merge-conflict markers

`serde_yaml` is deprecated and `serde_yml` is unmaintained and unsound (RUSTSEC-2025-0068). Both are banned in `deny.toml`.

## Options considered
1. **`serde-saphyr` 1.3** — actively maintained (1.3.0 released 2026-09).
   - `SerializerOptions { quote_all, prefer_block_scalars, … }`.
   - Wrapper types: `LitStr` for `|` blocks and `FlowMap` for one-line rows.
   - Errors carry `Location` (line/column) plus a rendered source snippet.
2. **`serde_norway` 0.9** — maintained fork of `serde_yaml`, last released 2024-12.
   - No option to force quoting. It writes `no`, `yes`, `on` and `2026-10-05` unquoted. Those are strings under YAML 1.2, but YAML 1.1 tools and many editors read them as bool/date.
   - No control over block vs flow style per field.

A scratch comparison was run against both, using the same structs and inputs.

## Decision
Use **`serde-saphyr`**, with `quote_all: true` and `prefer_block_scalars: false`. Style is controlled per field:
- `model::style::multiline` writes strings that contain a newline as `|` literal blocks, with chomping chosen automatically: `|`, `|-` or `|+`. Single-line strings stay quoted scalars.
- `model::style::flow_rows` writes table rows (headers, params, path params, form fields, extract, assert) as one-line flow maps, `- {name: 'X', value: 'y', enabled: true}`, so each row is one diff line.
- Every struct and list item has `#[serde(flatten)] extra: IndexMap<String, serde_json::Value>`, using `preserve_order`.

`serde_norway` is **not** kept as a fallback dependency. If serde-saphyr ever had to be replaced, the style hints would need reimplementing anyway.

Code: `ironcall-core::model` (schema types) and `ironcall-store::yaml` (`from_str`, `to_string`, `schema_version`). Fixtures are in `tests/fixtures/yaml/{canonical,handwritten,invalid}`.

## Findings

**Works (tested):**
- Byte-identical re-save for 18 canonical fixtures: every body type, every auth type, unicode, an empty body, a 180KB body, tricky strings, unknown keys at five levels (top, row, auth, body, settings), and a body without a trailing newline (`|-`).
- Handwritten files (the PLAN.md §4.1/§4.2 examples, with comments, double quotes and flow maps) normalise in one save, and stay stable after that. The §4.1 example normalises exactly to `canonical/request-full.yaml`.
- **Typed `String` fields keep the source text even when unquoted**, including through `flatten`: `value: 0123` → `"0123"`, `value: 1.0` → `"1.0"`, `name: no` → `"no"`.
- Body whitespace is preserved exactly: leading indentation, trailing blank lines, tabs, whitespace-only lines.
- A proptest round-trips 256 random strings with mixed unicode, newlines and tabs through name, description, header value and body, then re-saves byte-identically.
- CRLF input parses, and output is always LF.
- Duplicate keys are rejected with a line number.
- **Errors:**
  - Syntax errors give a line, column and snippet.
  - Missing fields, a bad ULID and an unknown `auth.type` all fail loudly instead of being ignored.
  - Merge-conflict markers are detected before parsing, giving "unresolved merge conflict (marker at line N)". The parser's own message for these ("simple key expected ':'") was confusing.
- Blank lines inside `|` blocks are written without trailing whitespace. The serializer emits indentation-only lines there, so `to_string` blanks them out and re-parses to confirm the value is unchanged. If it would change (content with a line of only spaces), the original output is kept.

**Limitations (accepted; document in the file-format reference):**
1. **Comments are lost** on save. `serde-saphyr` has `Commented<T>` for writing comments, but there's no comment-preserving read. A comment-preserving CST writer is on the post-V1 list.
2. **Hand formatting is normalised** on the first save:
   - double quotes become single quotes
   - block-style list items become flow rows
   - `auth: {type: inherit}` and empty sections are dropped
   - an explicit default such as `enabled: true` is kept
3. **Strings with a backslash or single quote are double-quoted with escapes.** For example, regex `code=(\d+)` is written as `"code=(\\d+)"`. This is correct and stable, but a little less readable.
4. **Unknown keys move after the known keys** within their map. Their **unquoted** scalars keep YAML 1.1 typing: `no` → `false`, `0123` → `123.0`. Keys written by a future IronCall version are always quoted, so this only affects hand-added keys. Unknown keys must have string keys.
5. **Variables written as unquoted floats** in hand-edited files are re-formatted (`ratio: 1.50` → `'1.5'`). Variables are deserialized through `deserialize_any`, which yields an `f64`. Unquoted integers and bools are kept exactly.
6. **`ItemId` errors read "invalid length"**, from the ULID parser. The line and column are correct; the wording could be friendlier.

## Consequences
- `yaml::to_string` requires `T: Serialize + DeserializeOwned + PartialEq`, for the whitespace check. Saving costs one extra parse, which is negligible: about 0.15s for the whole suite including the 180KB fixture.
- `ironcall-core` depends on `serde-saphyr`, but only for the `LitStr`/`FlowMap` style wrappers in `model::style`. Other serde formats such as IPC JSON see plain strings and maps, so the model can go straight to the frontend.
- Phase 2 builds on this: atomic writes, slugs, error nodes (using `YamlError::line()`), and in-memory migrations (using `yaml::schema_version`).
- **PLAN.md updates needed:**
  - §4.1/§4.2 examples: show the canonical form (single quotes, flow rows, inherit omitted)
  - §4.4: list limitations 2–5
  - §3.3: remove `serde_norway` as a fallback
  - §4: add `schema` values for collection/folder (`ironcall/collection@1`, `ironcall/folder@1`)
