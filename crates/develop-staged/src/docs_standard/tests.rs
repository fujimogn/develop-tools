use super::*;

fn errors(findings: &[Finding]) -> Vec<&Finding> {
    findings
        .iter()
        .filter(|finding| finding.kind == FindingKind::Error)
        .collect()
}

#[test]
fn common_limits_are_single_public_contract() {
    assert_eq!(TODO_MAX_BYTES, 36_000);
    assert_eq!(CHANGELOG_MAX_BYTES, 120_000);
    assert_eq!(TODO_ITEM_WARNING_BYTES, 500);
    assert_eq!(CHANGELOG_ARCHIVE_TARGET_PERCENT, 80);
}

#[test]
fn changelog_accepts_exact_byte_cap_and_rejects_one_more_byte() {
    let prefix =
        "# Changelog\n> 古い分は git 履歴に任せる。\n\n## 2026-10\n- **成果（2026-10-14）**: ";
    let exact = format!(
        "{prefix}{}\n",
        "x".repeat(CHANGELOG_MAX_BYTES - prefix.len() - 1)
    );
    assert_eq!(exact.len(), CHANGELOG_MAX_BYTES);
    assert!(errors(&inspect_changelog(exact.as_bytes())).is_empty());

    let over = format!("{exact}x");
    assert_eq!(over.len(), CHANGELOG_MAX_BYTES + 1);
    assert!(
        errors(&inspect_changelog(over.as_bytes()))
            .iter()
            .any(|finding| { finding.message.contains("上限") })
    );
}

#[test]
fn todo_rejects_completed_items_and_warns_on_large_items() {
    let large = "x".repeat(TODO_ITEM_WARNING_BYTES);
    let content = format!("# TODO\n- [x] 完了した作業\n- [ ] {large}\n");

    let findings = inspect_todo(content.as_bytes());

    assert!(
        errors(&findings)
            .iter()
            .any(|finding| { finding.line == Some(2) && finding.message.contains("未完了") })
    );
    assert!(findings.iter().any(|finding| {
        finding.kind == FindingKind::Warning
            && finding.line == Some(3)
            && finding.message.contains("500")
    }));
}

#[test]
fn todo_item_warning_counts_utf8_body_without_its_final_newline() {
    let single_line = format!("- {}", "あ".repeat((TODO_ITEM_WARNING_BYTES - 2) / 3));
    assert_eq!(single_line.len(), TODO_ITEM_WARNING_BYTES);
    let continued = format!(
        "- 現在地\n  {}",
        "次".repeat((TODO_ITEM_WARNING_BYTES - "- 現在地".len() - 1 - 2) / 3)
    );
    assert_eq!(continued.len(), TODO_ITEM_WARNING_BYTES);

    for item in [&single_line, &continued] {
        let content = format!("# TODO\n{item}\n");
        let findings = inspect_todo(content.as_bytes());
        assert!(
            !findings
                .iter()
                .any(|finding| { finding.kind == FindingKind::Warning && finding.line == Some(2) })
        );
    }

    let over_limit = format!("{continued}x");
    assert_eq!(over_limit.len(), TODO_ITEM_WARNING_BYTES + 1);
    let content = format!("# TODO\n{over_limit}\n");
    let findings = inspect_todo(content.as_bytes());
    assert!(findings.iter().any(|finding| {
        finding.kind == FindingKind::Warning
            && finding.line == Some(2)
            && finding
                .message
                .contains(&format!("{} bytes", TODO_ITEM_WARNING_BYTES + 1))
    }));
}

#[test]
fn todo_rejects_bytes_over_the_common_limit() {
    let content = format!("{}\n", "x".repeat(TODO_MAX_BYTES));
    let findings = inspect_todo(content.as_bytes());

    assert!(
        errors(&findings)
            .iter()
            .any(|finding| finding.message.contains(&TODO_MAX_BYTES.to_string()))
    );
}

#[test]
fn changelog_accepts_the_standard_shape_and_month_order() {
    let content = "# Changelog\n> 古い分は git 履歴に任せる。\n\n## 2026-09\n- **成果（2026-09-27）**: 要約\n\n## 2026-08\n- **成果（2026-08-20、補足）**: 要約\n";

    assert!(inspect_changelog(content.as_bytes()).is_empty());
}

#[test]
fn changelog_rejects_a_daily_subheading_but_ignores_fenced_examples() {
    let content = "# Changelog\n> 古い分は git 履歴に任せる。\n\n## 2026-10\n### 2026-10-04\n- **成果（2026-10-04）**: 要約\n";
    let findings = inspect_changelog(content.as_bytes());
    assert!(errors(&findings).iter().any(|finding| {
        finding.line == Some(5)
            && finding.message == "CHANGELOG.md の見出しは ## YYYY-MM だけにしてください"
    }));

    let fenced = "# Changelog\n> 古い分は git 履歴に任せる。\n\n## 2026-10\n```markdown\n### 2026-10-04\n```\n- **成果（2026-10-04）**: 要約\n";
    assert!(errors(&inspect_changelog(fenced.as_bytes())).is_empty());
}

#[test]
fn changelog_rejects_an_item_whose_date_does_not_match_heading() {
    let content = "# Changelog\n> 古い分は git 履歴に任せる。\n\n## 2026-09\n- **成果（2026-08-27）**: 要約\n";

    let findings = inspect_changelog(content.as_bytes());

    assert!(
        errors(&findings)
            .iter()
            .any(|finding| { finding.line == Some(5) && finding.message.contains("年月") })
    );
}

#[test]
fn changelog_rejects_an_impossible_calendar_date() {
    let content = "# Changelog\n> 古い分は git 履歴に任せる。\n\n## 2026-09\n- **成果（2026-09-31）**: 要約\n";

    let findings = inspect_changelog(content.as_bytes());

    assert!(
        errors(&findings)
            .iter()
            .any(|finding| finding.line == Some(5))
    );
}

#[test]
fn changelog_rejects_a_month_outside_the_calendar() {
    let content = "# Changelog\n> 古い分は git 履歴に任せる。\n\n## 2026-13\n- **成果（2026-13-01）**: 要約\n";

    let findings = inspect_changelog(content.as_bytes());

    assert!(
        errors(&findings)
            .iter()
            .any(|finding| finding.message.contains("見出しは ## YYYY-MM"))
    );
}

#[test]
fn changelog_warns_on_a_large_top_level_item() {
    let content = format!(
        "# Changelog\n> 古い分は git 履歴に任せる。\n\n## 2026-09\n- **成果（2026-09-27）**: {}\n",
        "x".repeat(TODO_ITEM_WARNING_BYTES)
    );

    let findings = inspect_changelog(content.as_bytes());

    assert!(
        findings
            .iter()
            .any(|finding| { finding.kind == FindingKind::Warning && finding.line == Some(5) })
    );
}

#[test]
fn changelog_month_end_item_byte_count_stops_before_next_month_heading() {
    let prefix = "- **成果（2026-09-27）**: ";
    let first_line = format!("{prefix}要約");
    let item_bytes = 490;
    let continuation_bytes = item_bytes - first_line.len() - 1 - 2;
    let item = format!("{first_line}\n  {}", "x".repeat(continuation_bytes));
    assert_eq!(item.len(), item_bytes);

    let content = format!(
        "# Changelog\n> 古い分は git 履歴に任せる。\n\n## 2026-09\n{item}\n\n## 2026-08\n- **成果（2026-08-27）**: 次月の成果\n"
    );

    let findings = inspect_changelog(content.as_bytes());

    assert!(errors(&findings).is_empty());
    assert!(
        !findings
            .iter()
            .any(|finding| { finding.kind == FindingKind::Warning && finding.line == Some(5) })
    );
}

#[test]
fn changelog_item_warning_counts_utf8_body_without_its_final_newline() {
    let prefix = "- **成果（2026-09-27）**: ";
    let summary_bytes = TODO_ITEM_WARNING_BYTES - prefix.len();
    let summary = format!(
        "{}{}",
        "あ".repeat(summary_bytes / 3),
        "x".repeat(summary_bytes % 3)
    );
    let item = format!("{prefix}{summary}");
    assert_eq!(item.len(), TODO_ITEM_WARNING_BYTES);

    let content = format!("# Changelog\n> 古い分は git 履歴に任せる。\n\n## 2026-09\n{item}\n");
    let findings = inspect_changelog(content.as_bytes());
    assert!(
        !findings
            .iter()
            .any(|finding| { finding.kind == FindingKind::Warning && finding.line == Some(5) })
    );

    let over_limit = format!("{item}x");
    assert_eq!(over_limit.len(), TODO_ITEM_WARNING_BYTES + 1);
    let content =
        format!("# Changelog\n> 古い分は git 履歴に任せる。\n\n## 2026-09\n{over_limit}\n");
    let findings = inspect_changelog(content.as_bytes());
    assert!(findings.iter().any(|finding| {
        finding.kind == FindingKind::Warning
            && finding.line == Some(5)
            && finding
                .message
                .contains(&format!("{} bytes", TODO_ITEM_WARNING_BYTES + 1))
    }));
}

#[test]
fn archive_keeps_protected_items_after_a_fenced_month_heading() {
    let protected_item = format!("- **残す成果（2026-10-04）**: {}\n", "x".repeat(100_000));
    let content = format!(
        "# Changelog\n> 古い分は git 履歴に任せる\n\n## 2026-10\n- **先頭（2026-10-04）**: 要約\n```markdown\n## 2026-05\n```\n{protected_item}## 2026-09\n- **古い成果（2026-09-01）**: {}\n",
        "y".repeat(25_000)
    );
    let plan = plan_changelog_archive(&content, "2026-10", "abc123").unwrap();
    assert_eq!(plan.archived_months, ["2026-09"]);
    assert!(plan.content.contains(&protected_item));
    assert!(plan.content.contains("```markdown\n## 2026-05\n```\n"));
}

#[test]
fn archive_is_a_noop_when_changelog_is_within_the_limit() {
    let content = "# Changelog\n> 古い分は git 履歴に任せる。\n\n## 2026-09\n- **成果（2026-09-27）**: 要約\n";

    let plan = plan_changelog_archive(content, "2026-09", "abc123").unwrap();

    assert_eq!(plan.content, content);
    assert!(plan.archived_months.is_empty());
    assert_eq!(plan.bytes_before, content.len());
    assert_eq!(plan.bytes_after, content.len());
}

#[test]
fn archive_removes_oldest_months_and_records_range_and_sha() {
    let block = |month: &str| {
        format!(
            "## {month}\n- **成果（{month}-01）**: {}\n\n",
            "x".repeat(55_000)
        )
    };
    let content = format!(
        "# Changelog\n> 古い分は git 履歴に任せる。\n\n{}{}{}",
        block("2026-09"),
        block("2026-08"),
        block("2026-07")
    );

    let plan = plan_changelog_archive(&content, "2026-09", "deadbeef").unwrap();

    assert_eq!(plan.archived_months, vec!["2026-07", "2026-08"]);
    assert!(plan.content.contains("## 2026-09"));
    assert!(!plan.content.contains("## 2026-08"));
    assert!(
        plan.content
            .contains("> 2026-08 以前は git 履歴（退避直前の commit: deadbeef）")
    );
    assert!(plan.content.contains("deadbeef"));
    assert!(plan.bytes_after <= CHANGELOG_MAX_BYTES * CHANGELOG_ARCHIVE_TARGET_PERCENT / 100);
}

fn changelog_with_large_history(preamble: &str) -> String {
    let section =
        |month: &str, summary: &str| format!("## {month}\n- **成果（{month}-01）**: {summary}\n\n");
    let large_summary = "x".repeat(55_000);
    format!(
        "{preamble}{}{}{}{}",
        section("2026-10", "current"),
        section("2026-09", &large_summary),
        section("2026-08", &large_summary),
        section("2026-07", &large_summary)
    )
}

#[test]
fn archive_replaces_history_note_in_place_and_covers_the_newest_archived_month() {
    let content = changelog_with_large_history(
        "# Changelog\n\n説明文です。\n\n> 2026-07 以前は git 履歴（退避直前の commit: oldsha）\n\n",
    );

    let plan = plan_changelog_archive(&content, "2026-10", "deadbeef").unwrap();
    let expected_preamble = "# Changelog\n\n説明文です。\n\n> 2026-08 以前は git 履歴（退避直前の commit: deadbeef）\n\n";

    assert!(plan.content.starts_with(expected_preamble));
    assert_eq!(plan.archived_months, vec!["2026-07", "2026-08"]);
    assert_eq!(plan.content.matches("以前は git 履歴").count(), 1);
}

#[test]
fn archive_inserts_history_note_before_first_heading_when_missing() {
    let content = changelog_with_large_history("# Changelog\n\n説明文です。\n\n");

    let plan = plan_changelog_archive(&content, "2026-10", "deadbeef").unwrap();
    let expected_preamble = "# Changelog\n\n説明文です。\n\n> 2026-08 以前は git 履歴（退避直前の commit: deadbeef）\n\n";

    assert!(plan.content.starts_with(expected_preamble));
    assert_eq!(plan.content.matches("以前は git 履歴").count(), 1);
}

#[test]
fn archive_refuses_to_apply_when_current_month_alone_is_too_large() {
    let content = format!(
        "# Changelog\n> 古い分は git 履歴に任せる。\n\n## 2026-09\n- **成果（2026-09-01）**: {}\n\n## 2026-08\n- **成果（2026-08-01）**: old\n",
        "x".repeat(CHANGELOG_MAX_BYTES)
    );

    let error = plan_changelog_archive(&content, "2026-09", "deadbeef").unwrap_err();

    assert!(error.contains("当月の項目を畳む"));
}
