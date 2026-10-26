use sha2::{Digest, Sha256};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
fn write(root: &Path, path: &str, text: &str) {
    fs::write(root.join(path), text).unwrap();
}
fn document(n: u32) -> String {
    format!(
        "---\nstatus: accepted\ndate: 2026-10-27\n---\n\n# ADR-{n:04}: Decision\n\nChosen option: test\n- Good: checked\n- Bad: cost\n- Neutral: unchanged\n\n### Confirmation\n\nRun the fixture.\n"
    )
}
fn fixture() -> tempfile::TempDir {
    let d = tempfile::Builder::new()
        .prefix("develop-meta-inspect-adr-")
        .tempdir()
        .unwrap();
    for p in ["docs/adr", "scripts/develop-tools", ".config"] {
        fs::create_dir_all(d.path().join(p)).unwrap();
    }
    write(
        d.path(),
        ".config/develop-tools.json",
        r#"{"adrReservedNumbers":[]}"#,
    );
    write(d.path(), "docs/adr/adr-template.md", "template\n");
    write(
        d.path(),
        "scripts/develop-tools/adr-template.md",
        "template\n",
    );
    write(
        d.path(),
        "docs/adr/README.md",
        "0001-first.md\n0002-second.md\n0001-copy.md\n0003-third.md\n",
    );
    write(d.path(), "docs/adr/0001-first.md", &document(1));
    d
}
fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_inspect-adr"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}
fn check(root: &Path, code: i32, issue: &str) {
    let out = run(root, &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(code), "{stderr}");
    assert!(stderr.contains(issue), "missing {issue}: {stderr}");
}
#[test]
fn valid_worktree_needs_no_git() {
    let d = fixture();
    check(d.path(), 0, "");
}
#[test]
fn broken_documents_fail() {
    for (path, content, issue) in [
        ("docs/adr/0001-copy.md", document(1), "duplicate-adr-number"),
        (
            "docs/adr/0003-third.md",
            document(3),
            "nonsequential-adr-number",
        ),
        (
            "docs/adr/adr-template.md",
            "drift".into(),
            "adr-template-drift",
        ),
        (
            "docs/adr/0001-first.md",
            "broken".into(),
            "invalid-adr-structure",
        ),
    ] {
        let d = fixture();
        write(d.path(), path, &content);
        check(d.path(), 1, issue);
    }
}
#[test]
fn missing_readme_and_template_are_violations() {
    for (path, issue) in [
        ("docs/adr/README.md", "missing-adr-readme"),
        ("docs/adr/adr-template.md", "missing-adr-template"),
    ] {
        let d = fixture();
        fs::remove_file(d.path().join(path)).unwrap();
        check(d.path(), 1, issue);
    }
}
#[test]
fn missing_inputs_and_bad_arguments_return_two() {
    for path in [
        ".config/develop-tools.json",
        "scripts/develop-tools/adr-template.md",
    ] {
        let d = fixture();
        fs::remove_file(d.path().join(path)).unwrap();
        check(d.path(), 2, "error:");
    }
    let d = fixture();
    assert_eq!(run(d.path(), &["--unknown"]).status.code(), Some(2));
    assert!(run(d.path(), &["--help"]).status.success());
    for text in ["{}", "invalid JSON"] {
        write(d.path(), ".config/develop-tools.json", text);
        check(d.path(), 2, "error:");
    }
}
#[test]
fn reserved_gap_is_allowed_but_reuse_fails() {
    let d = fixture();
    write(
        d.path(),
        ".config/develop-tools.json",
        r#"{"adrReservedNumbers":[1]}"#,
    );
    fs::remove_file(d.path().join("docs/adr/0001-first.md")).unwrap();
    write(d.path(), "docs/adr/0002-second.md", &document(2));
    check(d.path(), 0, "");
    write(d.path(), "docs/adr/0001-first.md", &document(1));
    check(d.path(), 1, "reused-reserved-adr-number");
}
#[test]
fn archive_hash_and_sanitized_contract_are_enforced() {
    for sanitized in [false, true] {
        let d = fixture();
        let numbers = if sanitized { "[1]" } else { "[]" };
        write(
            d.path(),
            ".config/develop-tools.json",
            &format!(r#"{{"adrMigratedThrough":1,"adrSanitizedNumbers":{numbers}}}"#),
        );
        let hash = Sha256::digest(b"record\n")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let archive = if sanitized {
            format!(
                "<!-- Private legacy source SHA-256: {} -->\n<!-- Public sanitized record SHA-256: {hash} -->\n\n## Sanitized Original Record\n\n~~~~markdown\nrecord\n~~~~\n",
                "0".repeat(64)
            )
        } else {
            format!(
                "<!-- Legacy source SHA-256: {hash} -->\n\n## Original Record\n\n~~~~markdown\nrecord\n~~~~\n"
            )
        };
        let text = document(1) + &archive;
        write(d.path(), "docs/adr/0001-first.md", &text);
        check(d.path(), 0, "");
        write(
            d.path(),
            "docs/adr/0001-first.md",
            &text.replace("\nrecord\n", "\ntampered\n"),
        );
        check(d.path(), 1, "invalid-legacy-archive");
    }
}

#[test]
fn unreadable_adr_input_returns_two() {
    let d = fixture();
    fs::create_dir(d.path().join("docs/adr/0002-second.md")).unwrap();
    check(d.path(), 2, "error:");
}

#[test]
fn inspection_preserves_worktree_bytes() {
    let d = fixture();
    let paths = [
        ".config/develop-tools.json",
        "docs/adr/README.md",
        "docs/adr/0001-first.md",
        "docs/adr/adr-template.md",
        "scripts/develop-tools/adr-template.md",
    ];
    let before: Vec<_> = paths
        .iter()
        .map(|p| fs::read(d.path().join(p)).unwrap())
        .collect();
    check(d.path(), 0, "");
    for (path, bytes) in paths.iter().zip(before) {
        assert_eq!(fs::read(d.path().join(path)).unwrap(), bytes);
    }
}
