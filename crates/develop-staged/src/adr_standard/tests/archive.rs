use super::*;

/// 移行対象 ADR に原文 archive が無ければ拒否する。
#[test]
fn a_migrated_adr_without_an_archive_is_rejected() {
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
        "- [0001](./0001-decision.md)\n",
    );

    let issues = inspect_root(&root, Some(1), &[], &[]);
    assert!(
        codes(&issues).contains(&"invalid-legacy-archive"),
        "{issues:?}"
    );
}

/// **明示された公開 ADR だけ** sanitized archive を許可する。
#[test]
fn only_allowlisted_numbers_may_use_a_sanitized_archive() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    write(
        &root,
        "docs/adr/0001-decision.md",
        &sanitized_legacy_adr("0001", "# 公開用\n\n伏せ字済み\n"),
    );
    write(
        &root,
        "docs/adr/README.md",
        "- [0001](./0001-decision.md)\n",
    );

    // allowlist に入っていれば通る。
    let allowed = inspect_root(&root, Some(1), &[], &[1]);
    assert!(
        !codes(&allowed).contains(&"invalid-legacy-archive"),
        "{allowed:?}"
    );

    // 入っていなければ通常契約で判定され、archive が無いものとして落ちる。
    let denied = inspect_root(&root, Some(1), &[], &[]);
    assert!(
        codes(&denied).contains(&"invalid-legacy-archive"),
        "allowlist 外の sanitized archive は従来契約違反: {denied:?}"
    );
}

/// sanitized archive の公開本文を改変したら digest 不一致で落とす。
#[test]
fn tampering_with_a_sanitized_record_is_rejected() {
    let root = temp_root();
    write(&root, "docs/adr/adr-template.md", TEMPLATE);
    let tampered =
        sanitized_legacy_adr("0001", "# 公開用\n\n伏せ字済み\n").replace("伏せ字済み", "改ざん");
    write(&root, "docs/adr/0001-decision.md", &tampered);
    write(
        &root,
        "docs/adr/README.md",
        "- [0001](./0001-decision.md)\n",
    );

    let issues = inspect_root(&root, Some(1), &[], &[1]);
    assert!(
        codes(&issues).contains(&"invalid-legacy-archive"),
        "{issues:?}"
    );
}
