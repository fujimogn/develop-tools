use crate::managed_block::{self, BlockState, MarkerFormat};

const FORMAT: MarkerFormat = MarkerFormat::new("test-block", b"");

#[test]
fn reports_missing_current_and_stale_while_preserving_outside_bytes() {
    let source = b"managed v1\n";
    let prefix = b"user bytes\r\n";
    let missing = managed_block::inspect(FORMAT, source, prefix).unwrap();
    assert_eq!(missing, BlockState::Missing);

    let plan = managed_block::plan(FORMAT, source, prefix, None).unwrap();
    assert_eq!(plan.state(), BlockState::Missing);
    assert_eq!(
        managed_block::apply(&plan, source, prefix).unwrap(),
        plan.updated_target()
    );

    let mut target = plan.updated_target().to_vec();
    target.extend_from_slice(b"tail remains\r\n");
    assert_eq!(
        managed_block::inspect(FORMAT, source, &target).unwrap(),
        BlockState::Current
    );

    let next_source = b"managed v2\n";
    assert_eq!(
        managed_block::inspect(FORMAT, next_source, &target).unwrap(),
        BlockState::Stale
    );
    let next = managed_block::plan(FORMAT, next_source, &target, None).unwrap();
    let applied = managed_block::apply(&next, next_source, &target).unwrap();
    assert!(applied.starts_with(prefix));
    assert!(applied.ends_with(b"tail remains\r\n"));
    assert!(
        applied
            .windows(next_source.len())
            .any(|part| part == next_source)
    );
}

#[test]
fn edited_hash_and_apply_time_source_or_target_changes_are_refused() {
    let source = b"managed v1\n";
    let initial = b"user prefix\n";
    let plan = managed_block::plan(FORMAT, source, initial, None).unwrap();

    let changed_target = b"user prefix changed\n";
    assert_eq!(
        managed_block::apply(&plan, source, changed_target),
        Err("managed_block_target_changed")
    );
    assert_eq!(
        managed_block::apply(&plan, b"managed v2\n", initial),
        Err("managed_block_source_changed")
    );

    let mut edited = plan.updated_target().to_vec();
    let body_at = edited
        .windows(b"managed v1".len())
        .position(|part| part == b"managed v1")
        .unwrap();
    edited[body_at] = b'M';
    assert_eq!(
        managed_block::inspect(FORMAT, source, &edited).unwrap(),
        BlockState::Edited
    );
    assert_eq!(
        managed_block::plan(FORMAT, source, &edited, None).err(),
        Some("managed_block_edited")
    );
}

#[test]
fn malformed_duplicate_or_reversed_markers_are_rejected() {
    let source = b"managed v1\n";
    let rendered = managed_block::plan(FORMAT, source, b"", None)
        .unwrap()
        .updated_target()
        .to_vec();
    let begin_end = rendered
        .windows(b"<!-- END".len())
        .position(|part| part == b"<!-- END")
        .unwrap();
    let mut missing_end = rendered.clone();
    missing_end.truncate(begin_end);
    assert_eq!(
        managed_block::inspect(FORMAT, source, &missing_end),
        Err("managed_block_marker_invalid")
    );

    let mut duplicate = rendered.clone();
    duplicate.extend_from_slice(&rendered);
    assert_eq!(
        managed_block::inspect(FORMAT, source, &duplicate),
        Err("managed_block_marker_invalid")
    );

    let end_line = &rendered[begin_end..];
    let begin_line_end = rendered.iter().position(|byte| *byte == b'\n').unwrap() + 1;
    let mut reversed = end_line.to_vec();
    reversed.extend_from_slice(&rendered[begin_line_end..begin_end]);
    reversed.extend_from_slice(&rendered[..begin_line_end]);
    assert_eq!(
        managed_block::inspect(FORMAT, source, &reversed),
        Err("managed_block_marker_invalid")
    );

    let mut malformed_hash = rendered;
    let hash_start = b"sha256=".len();
    let begin_line = malformed_hash
        .windows(b"sha256=".len())
        .position(|part| part == b"sha256=")
        .unwrap()
        + hash_start;
    malformed_hash[begin_line..begin_line + 64].fill(b'0');
    assert_eq!(
        managed_block::inspect(FORMAT, source, &malformed_hash),
        Ok(BlockState::Edited)
    );
    assert_eq!(
        managed_block::plan(FORMAT, source, &malformed_hash, None).err(),
        Some("managed_block_edited")
    );
}

#[test]
fn marker_prefix_supports_yaml_comments_without_parsing_yaml() {
    const YAML_FORMAT: MarkerFormat = MarkerFormat::new("lefthook:docs-structure", b"# ");
    let source = b"    docs-structure:\n      run: echo ok\n";
    let target = b"pre-commit:\n  commands:\n";
    let plan = managed_block::plan(YAML_FORMAT, source, target, None).unwrap();
    let output = managed_block::apply(&plan, source, target).unwrap();

    assert!(
        output
            .windows(b"# <!-- BEGIN develop-tools:".len())
            .any(|part| { part == b"# <!-- BEGIN develop-tools:" })
    );
    assert_eq!(
        managed_block::inspect(YAML_FORMAT, source, &output),
        Ok(BlockState::Current)
    );
}

#[test]
fn intact_range_covers_only_a_block_whose_hash_matches() {
    let source = b"managed v1\n";
    let prefix = b"user bytes\n";
    let plan = managed_block::plan(FORMAT, source, prefix, None).unwrap();
    let mut target = plan.updated_target().to_vec();
    target.extend_from_slice(b"tail\n");

    let range = managed_block::intact_range(FORMAT, &target).expect("intact block");
    assert!(target[range.clone()].starts_with(b"<!-- BEGIN develop-tools:test-block v1 sha256="));
    assert!(target[range.clone()].ends_with(b"<!-- END develop-tools:test-block -->\n"));
    // 追加時に補う空行は区画の外（配布先の本文側）に数える。
    assert_eq!(&target[..range.start], b"user bytes\n\n");
    assert_eq!(&target[range.end..], b"tail\n");

    // 区画が無い、本文を書き換えた、marker が片側だけ、のどれも除外の対象にしない。
    assert_eq!(managed_block::intact_range(FORMAT, prefix), None);
    let mut edited = target.clone();
    let at = edited
        .windows(7)
        .position(|part| part == b"managed")
        .unwrap();
    edited[at] = b'M';
    assert_eq!(managed_block::intact_range(FORMAT, &edited), None);
    let end_at = target
        .windows(8)
        .position(|part| part == b"<!-- END")
        .unwrap();
    assert_eq!(managed_block::intact_range(FORMAT, &target[..end_at]), None);
}

/// 公開する道具が知らない名前空間。呼出側が受け入れを渡したときだけ読む。
const PRIOR_NAMESPACE: &str = "prior-tools";
const PRIOR_FORMAT: MarkerFormat = FORMAT.accepting(&[PRIOR_NAMESPACE]);

fn rendered_in(namespace: &str, source: &[u8]) -> String {
    let new = managed_block::plan(FORMAT, source, b"user\n", None)
        .unwrap()
        .updated_target()
        .to_vec();
    String::from_utf8(new).unwrap().replace(
        "develop-tools:test-block",
        &format!("{namespace}:test-block"),
    )
}

#[test]
fn unaccepted_namespace_region_is_plain_text() {
    let source = b"managed v1\n";
    let prior = rendered_in(PRIOR_NAMESPACE, source);
    assert_eq!(
        managed_block::inspect(FORMAT, source, prior.as_bytes()),
        Ok(BlockState::Missing)
    );
    assert_eq!(managed_block::intact_range(FORMAT, prior.as_bytes()), None);
    assert_eq!(
        managed_block::managed_range(FORMAT, prior.as_bytes()),
        Ok(None)
    );
    assert_eq!(
        managed_block::uses_legacy_namespace(FORMAT, prior.as_bytes()),
        Ok(false)
    );
    let appended = managed_block::plan(FORMAT, source, prior.as_bytes(), None).unwrap();
    assert_eq!(appended.state(), BlockState::Missing);
    let updated = String::from_utf8(appended.updated_target().to_vec()).unwrap();
    assert!(updated.starts_with(&prior), "旧い区画は本文として残る");
    assert!(updated.ends_with(&rendered_in("develop-tools", source)["user\n".len()..]));
    // 受け入れない名前空間の marker を含む本文は、配る区画の本文として通る
    assert_eq!(
        managed_block::inspect(FORMAT, prior.as_bytes(), b""),
        Ok(BlockState::Missing)
    );
}

#[test]
fn accepted_namespace_is_read_and_rewritten_to_the_current_one() {
    let source = b"managed v1\n";
    let current = rendered_in("develop-tools", source);
    let prior = rendered_in(PRIOR_NAMESPACE, source);
    for target in [prior.as_bytes(), current.as_bytes()] {
        assert_eq!(
            managed_block::inspect(PRIOR_FORMAT, source, target),
            Ok(BlockState::Current)
        );
        assert!(managed_block::intact_range(PRIOR_FORMAT, target).is_some());
    }
    assert_eq!(
        managed_block::uses_legacy_namespace(PRIOR_FORMAT, prior.as_bytes()),
        Ok(true)
    );
    assert_eq!(
        managed_block::uses_legacy_namespace(PRIOR_FORMAT, current.as_bytes()),
        Ok(false)
    );
    let migrated = managed_block::plan(PRIOR_FORMAT, source, prior.as_bytes(), None).unwrap();
    assert_eq!(migrated.state(), BlockState::Stale);
    assert_eq!(migrated.updated_target(), current.as_bytes());
    let mixed = prior.replace(
        &format!("<!-- END {PRIOR_NAMESPACE}:"),
        "<!-- END develop-tools:",
    );
    assert_eq!(
        managed_block::inspect(PRIOR_FORMAT, source, mixed.as_bytes()),
        Err("managed_block_marker_invalid")
    );
    assert_eq!(
        managed_block::inspect(PRIOR_FORMAT, prior.as_bytes(), b""),
        Err("managed_block_source_invalid")
    );
}

/// 手で直した区画は `plan` が拒み、利用者が明示したときだけ `plan_overwrite` が区画の外を保って置き換える。
#[test]
fn overwrite_replaces_only_an_edited_block_and_keeps_outside_bytes() {
    let source = b"managed v1\n";
    let written = managed_block::plan(FORMAT, source, b"head\n", None).unwrap();
    let mut target = written.updated_target().to_vec();
    target.extend_from_slice(b"tail\n");
    let edited = String::from_utf8(target.clone())
        .unwrap()
        .replace("managed v1\n", "managed by hand\n")
        .into_bytes();
    assert_eq!(
        managed_block::plan(FORMAT, source, &edited, None).err(),
        Some("managed_block_edited")
    );

    let plan = managed_block::plan_overwrite(FORMAT, source, &edited).unwrap();
    assert_eq!(plan.state(), BlockState::Edited);
    assert_eq!(plan.updated_target(), target.as_slice());
    assert_eq!(
        managed_block::apply(&plan, source, &edited).unwrap(),
        target
    );

    // 手で直していない区画と、区画の無い target は通常の `plan` に任せる。
    assert_eq!(
        managed_block::plan_overwrite(FORMAT, source, &target).err(),
        Some("managed_block_not_edited")
    );
    assert_eq!(
        managed_block::plan_overwrite(FORMAT, source, b"head\n").err(),
        Some("managed_block_not_edited")
    );
}
