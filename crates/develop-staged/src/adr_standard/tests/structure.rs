use super::*;

#[test]
fn every_adr_needs_the_standard_structure_and_a_readme_entry() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    write(&root, "docs/adr/README.md", "# ADR\n");
    write(&root, "docs/adr/0002-new-decision.md", "# incomplete\n");

    let issues = inspect_root(&root, None, &[], &[]);
    assert!(
        codes(&issues).contains(&"invalid-adr-structure"),
        "{issues:?}"
    );
    assert!(codes(&issues).contains(&"unindexed-adr"), "{issues:?}");
}

#[test]
fn a_missing_template_is_reported() {
    let root = temp_root();
    let issues = inspect_root(&root, None, &[], &[]);
    assert!(
        codes(&issues).contains(&"missing-adr-template"),
        "{issues:?}"
    );
}

#[test]
fn a_template_that_differs_from_the_sot_is_drift() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", "# 別のテンプレート\n");

    let issues = inspect_root(&root, None, &[], &[]);
    assert!(codes(&issues).contains(&"adr-template-drift"), "{issues:?}");
}

/// template の placeholder（`{...}`）を残した新規 ADR は構造違反。
#[test]
fn a_new_adr_that_keeps_a_template_placeholder_is_rejected() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    write(
        &root,
        "docs/adr/README.md",
        "- [0001](./0001-decision.md)\n",
    );
    write(
        &root,
        "docs/adr/0001-decision.md",
        &filled_adr("0001", "decision").replace("context", "{背景}"),
    );

    let issues = inspect_root(&root, None, &[], &[]);
    assert!(
        codes(&issues).contains(&"invalid-adr-structure"),
        "{issues:?}"
    );
}

/// **コード例の波括弧は placeholder ではない。** code fence と inline code は判定から外す。
#[test]
fn braces_inside_code_examples_are_not_placeholders() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    write(
        &root,
        "docs/adr/README.md",
        "- [0001](./0001-decision.md)\n",
    );
    let with_code = filled_adr("0001", "decision").replace(
        "context",
        "設定例:\n\n```json\n{ \"key\": \"value\" }\n```\n\nインライン `{ a: 1 }` も同様。",
    );
    write(&root, "docs/adr/0001-decision.md", &with_code);

    let issues = inspect_root(&root, None, &[], &[]);
    assert!(
        !codes(&issues).contains(&"invalid-adr-structure"),
        "コード例の波括弧で誤検知している: {issues:?}"
    );
}

/// CRLF の新規 ADR も同じ構造として受け入れる。
#[test]
fn a_crlf_adr_is_accepted_as_the_same_structure() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    let crlf = filled_adr("0001", "decision").replace('\n', "\r\n");
    write(&root, "docs/adr/0001-decision.md", &crlf);
    write(
        &root,
        "docs/adr/README.md",
        "- [0001](./0001-decision.md)\n",
    );

    let issues = inspect_root(&root, None, &[], &[]);
    assert!(
        !codes(&issues).contains(&"invalid-adr-structure"),
        "CRLF を別構造とみなしている: {issues:?}"
    );
}

/// `status` と `date` は frontmatter の中に無ければならない。
#[test]
fn status_and_date_must_live_in_the_frontmatter() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    // frontmatter を落とし、本文へ status / date を置く。
    let body = filled_adr("0001", "decision");
    let without_frontmatter = body
        .split_once("---\n")
        .and_then(|(_, rest)| rest.split_once("---\n"))
        .map(|(_, rest)| format!("status: accepted\ndate: 2026-07-12\n{rest}"))
        .expect("split");
    write(&root, "docs/adr/0001-decision.md", &without_frontmatter);
    write(
        &root,
        "docs/adr/README.md",
        "- [0001](./0001-decision.md)\n",
    );

    let issues = inspect_root(&root, None, &[], &[]);
    assert!(
        codes(&issues).contains(&"invalid-adr-structure"),
        "{issues:?}"
    );
}
