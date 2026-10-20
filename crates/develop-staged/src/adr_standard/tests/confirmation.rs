use super::*;

/// 移行済み範囲より上の新規 ADR が Confirmation に移行 boilerplate を残すと違反。
#[test]
fn a_new_adr_with_the_migration_boilerplate_confirmation_is_a_violation() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    write(
        &root,
        "docs/adr/README.md",
        "- [0002](./0002-decision.md)\n",
    );
    write(
        &root,
        "docs/adr/0001-old.md",
        &legacy_adr("0001", "旧本文\n"),
    );
    write(
        &root,
        "docs/adr/0002-decision.md",
        &filled_adr("0002", "decision").replace("manual review", "移行前原文のSHA-256を照合する"),
    );
    write(
        &root,
        "docs/adr/README.md",
        "- [0001](./0001-old.md)\n- [0002](./0002-decision.md)\n",
    );

    let issues = inspect_root(&root, Some(1), &[], &[]);
    assert!(
        codes(&issues).contains(&"boilerplate-confirmation"),
        "{issues:?}"
    );
}

/// boilerplate を残していても `migratedThrough` 以下の legacy ADR は対象外。
#[test]
fn a_legacy_adr_below_the_boundary_may_keep_the_boilerplate() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    let legacy =
        legacy_adr("0001", "旧本文\n").replace("manual review", "移行前原文のSHA-256を照合する");
    write(&root, "docs/adr/0001-decision.md", &legacy);
    write(
        &root,
        "docs/adr/README.md",
        "- [0001](./0001-decision.md)\n",
    );

    let issues = inspect_root(&root, Some(1), &[], &[]);
    assert!(
        !codes(&issues).contains(&"boilerplate-confirmation"),
        "境界以下は対象外: {issues:?}"
    );
}

/// `migratedThrough` 未指定（未移行の新規 project）では全 ADR が boilerplate 検査の対象。
#[test]
fn without_a_boundary_every_adr_is_checked_for_boilerplate() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    write(
        &root,
        "docs/adr/0001-decision.md",
        &filled_adr("0001", "decision").replace("manual review", "移行前原文のSHA-256を照合する"),
    );
    write(
        &root,
        "docs/adr/README.md",
        "- [0001](./0001-decision.md)\n",
    );

    let issues = inspect_root(&root, None, &[], &[]);
    assert!(
        codes(&issues).contains(&"boilerplate-confirmation"),
        "{issues:?}"
    );
}

/// Confirmation 節が **HTML コメント先行 + boilerplate 本文**でも検出する。
///
/// template はコメントが先・本文が後の順なので、コメントで打ち切ると本文を見落とす。
#[test]
fn a_confirmation_with_a_leading_html_comment_is_still_detected() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    write(
        &root,
        "docs/adr/0001-decision.md",
        &filled_adr("0001", "decision").replace(
            "manual review",
            "<!-- どう検証したかを書く -->\n\n移行前原文のSHA-256を照合する",
        ),
    );
    write(
        &root,
        "docs/adr/README.md",
        "- [0001](./0001-decision.md)\n",
    );

    let issues = inspect_root(&root, None, &[], &[]);
    assert!(
        codes(&issues).contains(&"boilerplate-confirmation"),
        "{issues:?}"
    );
}

#[test]
fn an_adr_with_its_own_confirmation_is_not_a_violation() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    write(
        &root,
        "docs/adr/0001-decision.md",
        &filled_adr("0001", "decision")
            .replace("manual review", "`bun test bin/foo.test.ts` が緑であること"),
    );
    write(
        &root,
        "docs/adr/README.md",
        "- [0001](./0001-decision.md)\n",
    );

    let issues = inspect_root(&root, None, &[], &[]);
    assert!(
        !codes(&issues).contains(&"boilerplate-confirmation"),
        "{issues:?}"
    );
}
