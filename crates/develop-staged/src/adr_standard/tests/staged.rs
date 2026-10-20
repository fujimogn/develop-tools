use super::*;

#[test]
fn staged_inspect_rejects_a_tampered_legacy_original_record() {
    let root = temp_root();
    let mut body = legacy_adr("0001", "旧本文\n");
    body = body.replace("旧本文", "改ざん");
    let issues = inspect_staged(
        &staged_opts(&root),
        &validators(),
        &[StagedFile {
            name: "0001-first.md".into(),
            content: body,
            deleted: false,
        }],
        Some("- [0001](./0001-first.md)\n"),
    );
    assert_eq!(codes(&issues), ["invalid-legacy-archive"], "{issues:?}");
}

#[test]
fn staged_inspect_accepts_a_standard_new_adr_when_indexed() {
    let root = temp_root();
    let issues = inspect_staged(
        &staged_opts(&root),
        &validators(),
        &[StagedFile {
            name: "0002-new-decision.md".into(),
            content: filled_adr("0002", "decision"),
            deleted: false,
        }],
        Some("- [0002](./0002-new-decision.md)\n"),
    );
    assert!(issues.is_empty(), "{issues:?}");
}

#[test]
fn staged_inspect_rejects_reusing_a_reserved_number() {
    let root = temp_root();
    let reserved = [2];
    let base = staged_opts(&root);
    let options = Options {
        project_root: base.project_root,
        canonical_template: base.canonical_template,
        migrated_through: base.migrated_through,
        reserved_numbers: &reserved,
        sanitized_numbers: base.sanitized_numbers,
    };
    let issues = inspect_staged(
        &options,
        &validators(),
        &[StagedFile {
            name: "0002-new-decision.md".into(),
            content: filled_adr("0002", "decision"),
            deleted: false,
        }],
        Some("- [0002](./0002-new-decision.md)\n"),
    );
    assert!(
        codes(&issues).contains(&"reused-reserved-adr-number"),
        "{issues:?}"
    );
}

#[test]
fn staged_inspect_ignores_an_empty_stage() {
    let root = temp_root();
    let issues = inspect_staged(&staged_opts(&root), &validators(), &[], None);
    assert!(issues.is_empty(), "{issues:?}");
}

#[test]
fn staged_inspect_does_not_treat_a_worktree_readme_as_the_commit_index() {
    let root = temp_root();
    let issues = inspect_staged(
        &staged_opts(&root),
        &validators(),
        &[StagedFile {
            name: "0002-new-decision.md".into(),
            content: filled_adr("0002", "decision"),
            deleted: false,
        }],
        None,
    );
    assert!(
        codes(&issues).contains(&"unindexed-adr") || codes(&issues).contains(&"missing-adr-readme"),
        "{issues:?}"
    );
}
