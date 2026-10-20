use super::*;

#[test]
fn a_missing_adr_readme_is_reported() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);

    let issues = inspect_root(&root, None, &[], &[]);
    assert!(issues.iter().any(|issue| {
        issue.code == "missing-adr-readme" && issue.message == "docs/adr/README.md がありません"
    }));
}

/// README が同じ filename を複数回参照していても索引済みと扱う。
#[test]
fn multiple_readme_references_still_count_as_indexed() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    write(
        &root,
        "docs/adr/0001-decision.md",
        &filled_adr("0001", "decision"),
    );
    write(
        &root,
        "docs/adr/README.md",
        "- [0001](./0001-decision.md)\n\n再掲: [0001](./0001-decision.md)\n",
    );

    let issues = inspect_root(&root, None, &[], &[]);
    assert!(!codes(&issues).contains(&"unindexed-adr"), "{issues:?}");
}
