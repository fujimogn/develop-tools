use super::*;
use develop_adr::{validate_archive, validate_sanitized_archive};
use sha2::{Digest, Sha256};

pub(super) const TEMPLATE: &str = r#"---
status: proposed
date: YYYY-MM-DD
---

# ADR-NNNN: title

## Context and Problem Statement

context

## Decision Drivers

- driver

## Considered Options

- option

## Decision Outcome

Chosen option: option

### Consequences

- Good: result
- Bad: cost
- Neutral: effect

### Confirmation

manual review
"#;

pub(super) struct Root {
    _dir: tempfile::TempDir,
    path: std::path::PathBuf,
}

pub(super) fn temp_root() -> Root {
    let dir = tempfile::Builder::new()
        .prefix("adr-standard-")
        .tempdir()
        .expect("temp");
    let path = dir.path().to_path_buf();
    fs::create_dir_all(path.join("docs").join("adr")).expect("adr dir");
    Root { _dir: dir, path }
}

pub(super) fn write(root: &Root, rel: &str, body: &str) {
    let path = root.path.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(path, body).expect("write");
}

/// 標準テンプレートを埋めた新規 ADR。
pub(super) fn filled_adr(number: &str, title: &str) -> String {
    TEMPLATE
        .replace("YYYY-MM-DD", "2026-07-12")
        .replace("ADR-NNNN: title", &format!("ADR-{number}: {title}"))
}

fn digest(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    // sha2 0.11 の `finalize()` は `LowerHex` を持たない `Array` を返す。
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 移行済み ADR（標準構造 + Original Record）。
pub(super) fn legacy_adr(number: &str, original: &str) -> String {
    let standard = filled_adr(number, "decision");
    let archived = original.trim_end_matches('\n');
    format!(
        "{standard}\n<!-- Legacy source SHA-256: {} -->\n\n## Original Record\n\n~~~~markdown\n{archived}\n~~~~\n",
        digest(archived)
    )
}

/// 公開用に sanitize された移行済み ADR。
pub(super) fn sanitized_legacy_adr(number: &str, original: &str) -> String {
    let standard = filled_adr(number, "decision");
    let archived = original.trim_end_matches('\n');
    format!(
        "{standard}\n<!-- Private legacy source SHA-256: {} -->\n<!-- Public sanitized record SHA-256: {} -->\n\n## Sanitized Original Record\n\n~~~~markdown\n{archived}\n~~~~\n",
        "a".repeat(64),
        digest(archived)
    )
}

pub(super) fn validators() -> ArchiveValidators<'static> {
    ArchiveValidators {
        archive: &validate_archive,
        sanitized: &validate_sanitized_archive,
    }
}

pub(super) fn inspect_root(
    root: &Root,
    migrated: Option<u32>,
    reserved: &[u32],
    sanitized: &[u32],
) -> Vec<Issue> {
    inspect(
        &Options {
            project_root: &root.path,
            canonical_template: TEMPLATE,
            migrated_through: migrated,
            reserved_numbers: reserved,
            sanitized_numbers: sanitized,
        },
        &validators(),
    )
}

pub(super) fn codes(issues: &[Issue]) -> Vec<&str> {
    issues.iter().map(|issue| issue.code).collect()
}

pub(super) fn staged_opts(root: &Root) -> Options<'_> {
    Options {
        project_root: &root.path,
        canonical_template: TEMPLATE,
        migrated_through: Some(1),
        reserved_numbers: &[],
        sanitized_numbers: &[],
    }
}
