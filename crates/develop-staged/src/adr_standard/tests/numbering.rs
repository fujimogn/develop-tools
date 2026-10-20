use super::*;

fn number_paths(entries: &[(u32, &[&str])]) -> BTreeMap<u32, BTreeSet<String>> {
    entries
        .iter()
        .map(|(number, paths)| {
            (
                *number,
                paths.iter().map(|path| (*path).to_string()).collect(),
            )
        })
        .collect()
}

#[test]
fn transition_rejects_a_new_gap_but_ignores_existing_gaps() {
    let head = number_paths(&[
        (1, &["docs/adr/0001-first.md"]),
        (3, &["docs/adr/0003-third.md"]),
    ]);
    let index = number_paths(&[
        (1, &["docs/adr/0001-first.md"]),
        (4, &["docs/adr/0004-fourth.md"]),
    ]);
    let issues =
        inspect_number_set_transition(&head, &index, &["docs/adr/0004-fourth.md".into()], &[]);
    assert!(codes(&issues).contains(&"removed-adr-number"), "{issues:?}");
    assert!(
        codes(&issues).contains(&"nonsequential-adr-number"),
        "{issues:?}"
    );

    let unchanged = number_paths(&[
        (1, &["docs/adr/0001-first.md"]),
        (3, &["docs/adr/0003-third.md"]),
    ]);
    let issues = inspect_number_set_transition(
        &unchanged,
        &unchanged,
        &["docs/adr/0003-third.md".into()],
        &[],
    );
    assert!(issues.is_empty(), "既存欠番だけでは拒否しない: {issues:?}");
}

#[test]
fn transition_rejects_edit_to_an_existing_duplicate_but_ignores_unrelated_duplicate() {
    let head = number_paths(&[
        (1, &["docs/adr/0001-first.md", "docs/adr/0001-second.md"]),
        (2, &["docs/adr/0002-second.md"]),
    ]);
    let index = number_paths(&[
        (1, &["docs/adr/0001-first.md", "docs/adr/0001-second.md"]),
        (2, &["docs/adr/0002-second.md"]),
    ]);
    let unrelated =
        inspect_number_set_transition(&head, &index, &["docs/adr/0002-second.md".into()], &[]);
    assert!(unrelated.is_empty(), "無関係な既存重複: {unrelated:?}");

    // The path map is unchanged: this staged path represents a body-only edit
    // to one member of the pre-existing duplicate number.
    let edited_duplicate =
        inspect_number_set_transition(&head, &index, &["docs/adr/0001-first.md".into()], &[]);
    assert!(
        codes(&edited_duplicate).contains(&"duplicate-adr-number"),
        "{edited_duplicate:?}"
    );
}

#[test]
fn same_number_rename_preserves_the_number_and_reserved_holes_are_ignored() {
    let head = number_paths(&[(1, &["docs/adr/0001-first.md"])]);
    let index = number_paths(&[(1, &["docs/adr/0001-renamed.md"])]);
    let renamed = inspect_number_set_transition(
        &head,
        &index,
        &[
            "docs/adr/0001-first.md".into(),
            "docs/adr/0001-renamed.md".into(),
        ],
        &[],
    );
    assert!(renamed.is_empty(), "same number rename: {renamed:?}");

    let reserved = number_paths(&[(2, &["docs/adr/0002-second.md"])]);
    let issues = inspect_number_set_transition(
        &BTreeMap::new(),
        &reserved,
        &["docs/adr/0002-second.md".into()],
        &[1],
    );
    assert!(issues.is_empty(), "予約番号は欠番扱いしない: {issues:?}");
}

/// legacy 採番時点の欠番は再利用できない。
#[test]
fn reusing_a_reserved_number_is_rejected() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    write(
        &root,
        "docs/adr/0002-decision.md",
        &filled_adr("0002", "decision"),
    );
    write(
        &root,
        "docs/adr/README.md",
        "- [0002](./0002-decision.md)\n",
    );

    let issues = inspect_root(&root, None, &[2], &[]);
    assert!(
        codes(&issues).contains(&"reused-reserved-adr-number"),
        "{issues:?}"
    );
}

/// ADR は 1 から連続採番する。**予約番号は欠けていてよい**。
#[test]
fn adr_numbers_must_be_sequential_except_reserved_ones() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    write(
        &root,
        "docs/adr/0003-decision.md",
        &filled_adr("0003", "decision"),
    );
    write(
        &root,
        "docs/adr/README.md",
        "- [0003](./0003-decision.md)\n",
    );

    // 1 と 2 が欠番。
    let issues = inspect_root(&root, None, &[], &[]);
    let missing: Vec<&str> = codes(&issues)
        .into_iter()
        .filter(|code| *code == "nonsequential-adr-number")
        .collect();
    assert_eq!(missing.len(), 2, "{issues:?}");

    // 2 を予約にすると 1 だけが欠番として残る。
    let issues = inspect_root(&root, None, &[2], &[]);
    let missing: Vec<&str> = codes(&issues)
        .into_iter()
        .filter(|code| *code == "nonsequential-adr-number")
        .collect();
    assert_eq!(missing.len(), 1, "予約番号は欠番として数えない: {issues:?}");
}

#[test]
fn a_duplicate_number_is_reported() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    write(
        &root,
        "docs/adr/0001-first.md",
        &filled_adr("0001", "first"),
    );
    write(
        &root,
        "docs/adr/0001-second.md",
        &filled_adr("0001", "second"),
    );
    write(
        &root,
        "docs/adr/README.md",
        "- [0001](./0001-first.md)\n- [0001](./0001-second.md)\n",
    );

    let issues = inspect_root(&root, None, &[], &[]);
    let finding = issues
        .iter()
        .find(|issue| issue.code == "duplicate-adr-number")
        .expect("duplicate");
    assert!(finding.message.contains("ADR-0001"), "{}", finding.message);
}

#[test]
fn an_invalid_filename_is_reported() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    write(&root, "docs/adr/README.md", "# ADR\n");
    write(&root, "docs/adr/1-bad.md", "# x\n");
    write(&root, "docs/adr/0001_Bad_Case.md", "# x\n");

    let issues = inspect_root(&root, None, &[], &[]);
    let bad: Vec<&str> = codes(&issues)
        .into_iter()
        .filter(|code| *code == "invalid-adr-filename")
        .collect();
    assert_eq!(bad.len(), 2, "{issues:?}");
}

/// 移行済み ADR を厳格監査しつつ、予約欠番は連続採番から除外する。
#[test]
fn migrated_adrs_are_audited_strictly_and_reserved_gaps_are_excluded() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    write(
        &root,
        "docs/adr/0001-first.md",
        &legacy_adr("0001", "旧本文 1\n"),
    );
    write(
        &root,
        "docs/adr/0003-third.md",
        &legacy_adr("0003", "旧本文 3\n"),
    );
    write(
        &root,
        "docs/adr/README.md",
        "- [0001](./0001-first.md)\n- [0003](./0003-third.md)\n",
    );

    let issues = inspect_root(&root, Some(3), &[2], &[]);
    assert!(
        issues.is_empty(),
        "移行済み + 予約欠番なら指摘は出ない: {issues:?}"
    );
}
