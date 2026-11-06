use develop_release::ai_check::{Checks, DICTIONARY_MISSING, check_output, prepare};
use develop_release::release_notes::{
    ChangelogSection, ReleaseExtract, declared_notes, extract_changelog_section, extract_release,
    numbered_ai_input, prepare_notes, scrub_for_ai,
};
use develop_tools_core::develop_tools_manifest::NotesLanguage;
use island_llm::settings::{Audience, Settings};
use std::path::Path;
use std::process::Command;

fn git(root: &Path, args: &[&str]) -> String {
    let output = develop_tools_core::git(root)
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgSign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn commit(root: &Path, changelog: &str, message: &str) {
    std::fs::write(root.join("CHANGELOG.md"), changelog).unwrap();
    git(root, &["add", "CHANGELOG.md"]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            message,
        ],
    );
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(
        dir.path(),
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:example/project.git",
        ],
    );
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-09\n- **以前の項目**\n",
        "initial",
    );
    dir
}

fn notes(root: &Path, tag: &str) -> std::process::Output {
    notes_args(root, &[tag])
}

fn notes_args(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_release-notes"))
        .args(args)
        .current_dir(root)
        .output()
        .unwrap()
}

#[test]
fn previous_tag_is_required_even_for_the_first_release() {
    let dir = fixture();
    git(dir.path(), &["tag", "v0.1.0"]);
    let output = notes(dir.path(), "v0.1.0");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn declared_notes_allow_the_first_release_with_a_dated_section() {
    let dir = fixture();
    commit(
        dir.path(),
        "# Changelog\n\n## 0.1.0 (2026-11-05)\n\nInitial release.\n",
        "first release notes",
    );
    let declared = develop_release::release_notes::DeclaredNotes {
        plan: develop_release::release_notes::NotesPlan::Changelog,
        source: develop_tools_core::develop_tools_manifest::NotesSource::ChangelogAdditions,
        language: NotesLanguage::En,
    };
    assert!(prepare_notes(dir.path(), declared, "v0.1.0").is_ok());
}

#[test]
fn only_added_monthly_items_between_adjacent_tags_are_rendered() {
    let dir = fixture();
    git(dir.path(), &["tag", "v0.1.0"]);
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-09\n- **以前の項目**\n- **先の版の項目**\n",
        "first range",
    );
    git(dir.path(), &["tag", "v0.2.0"]);
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-09\n- **以前の項目**\n- **先の版の項目**\n- **今回の項目**\n\n```md\n- **fence 内**\n```\n\n## その他\n- **月外**\n\n## 2026-10\n- **今月の項目**\n",
        "second range",
    );
    git(dir.path(), &["tag", "v0.3.0"]);
    let output = notes(dir.path(), "v0.3.0");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "変更範囲: [v0.2.0...v0.3.0](https://github.com/example/project/compare/v0.2.0...v0.3.0)\n\n- **今回の項目**\n- **今月の項目**\n"
    );
}

#[test]
fn empty_additions_keep_only_the_compare_link() {
    let dir = fixture();
    git(dir.path(), &["tag", "v0.1.0"]);
    commit(
        dir.path(),
        "# Changelog\n\n## その他\n- **月外**\n",
        "no monthly additions",
    );
    git(dir.path(), &["tag", "v0.2.0"]);
    let output = notes(dir.path(), "v0.2.0");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "変更範囲: [v0.1.0...v0.2.0](https://github.com/example/project/compare/v0.1.0...v0.2.0)\n"
    );
}

#[test]
fn non_semver_previous_tag_fails_closed() {
    let dir = fixture();
    git(dir.path(), &["tag", "draft"]);
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-10\n- **項目**\n",
        "change",
    );
    git(dir.path(), &["tag", "v0.2.0"]);
    let output = notes(dir.path(), "v0.2.0");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn invocation_from_a_subdirectory_still_reads_the_root_changelog() {
    let dir = fixture();
    git(dir.path(), &["tag", "v0.1.0"]);
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-09\n- **以前の項目**\n- **追加項目**\n",
        "change",
    );
    git(dir.path(), &["tag", "v0.2.0"]);
    std::fs::create_dir(dir.path().join("tools")).unwrap();
    let output = notes(&dir.path().join("tools"), "v0.2.0");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("- **追加項目**")
    );
}

#[test]
fn a_signed_version_is_rejected_as_target_and_as_previous_tag() {
    let dir = fixture();
    git(dir.path(), &["tag", "v0.1.0"]);
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-10\n- **項目**\n",
        "change",
    );
    git(dir.path(), &["tag", "v+1.2.3"]);
    let output = notes(dir.path(), "v+1.2.3");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());

    commit(
        dir.path(),
        "# Changelog\n\n## 2026-10\n- **項目**\n- **次の項目**\n",
        "next",
    );
    git(dir.path(), &["tag", "v1.2.4"]);
    let output = notes(dir.path(), "v1.2.4");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn a_fence_whose_info_string_starts_with_the_other_marker_still_excludes_its_items() {
    let dir = fixture();
    git(dir.path(), &["tag", "v0.1.0"]);
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-09\n- **以前の項目**\n\n```~\n- **fence 内**\n```\n\n- **fence 後の項目**\n",
        "change",
    );
    git(dir.path(), &["tag", "v0.2.0"]);
    let output = notes(dir.path(), "v0.2.0");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "変更範囲: [v0.1.0...v0.2.0](https://github.com/example/project/compare/v0.1.0...v0.2.0)\n\n- **fence 後の項目**\n"
    );
}

#[test]
fn compact_metadata_and_parenthesized_dates_are_dropped() {
    assert_eq!(
        scrub_for_ai("- **版の刻印を直した** 2026-10-05/v0.5.3: 本文"),
        "- **版の刻印を直した**: 本文"
    );
    assert_eq!(
        scrub_for_ai("- **未リリースの項目** 2026-10-05 ⏳: 本文"),
        "- **未リリースの項目**: 本文"
    );
    assert_eq!(
        scrub_for_ai("- **日付だけの項目（2026-10-05）**: 本文"),
        "- **日付だけの項目**: 本文"
    );
}

#[test]
fn shas_lesson_ids_and_urls_are_dropped() {
    assert_eq!(
        scrub_for_ai(
            "- **検査を直した**: commit 63c0a289 の誤りを L59 と D11 に沿って直した。記録: [review](./docs/reviews/x.md)、詳細は https://github.com/example/project/pull/1 を参照"
        ),
        "- **検査を直した**: commit の誤りを と に沿って直した。記録: review、詳細は を参照"
    );
}

#[test]
fn ordinary_words_numbers_and_parentheses_stay() {
    let item = "- **CHANGELOG の上限を 60,000 B にした（島標準）**: v0.5.16 と ADR-0022 の S3 と deadline は残す";
    assert_eq!(scrub_for_ai(item), item);
}

#[test]
fn items_are_numbered_from_one() {
    assert_eq!(
        numbered_ai_input(&["- **一つ目**".to_owned(), "- **二つ目**".to_owned()]),
        "[1] - **一つ目**\n[2] - **二つ目**\n"
    );
}

#[test]
fn an_untagged_end_commit_extracts_the_same_items_as_the_tag() {
    let dir = fixture();
    git(dir.path(), &["tag", "v0.1.0"]);
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-10\n- **版 commit の項目**\n\n## 2026-09\n- **以前の項目**\n",
        "version commit",
    );
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    let extract = extract_release(dir.path(), &head, "v0.2.0", None).unwrap();
    assert_eq!(extract.previous, "v0.1.0");
    assert_eq!(extract.repo, "example/project");
    assert_eq!(extract.items, ["- **版 commit の項目**"]);

    git(dir.path(), &["tag", "v0.2.0"]);
    let output = notes(dir.path(), "v0.2.0");
    assert_eq!(String::from_utf8(output.stdout).unwrap(), extract.render());
}

#[test]
fn plain_numbers_stay_while_mixed_hex_shas_go() {
    assert_eq!(
        scrub_for_ai("上限を 1000000 件にし、20261021 の版を a1b2c3d で直した"),
        "上限を 1000000 件にし、20261021 の版を で直した"
    );
}

#[test]
fn a_link_target_with_parentheses_is_dropped_whole() {
    assert_eq!(
        scrub_for_ai("[説明](https://example.com/a(b)/internal) と [逃がし](./a\\)b.md) の後"),
        "説明 と 逃がし の後"
    );
}

/// 環境側の設定を file に書いて読む。`None` の欄は書かない。
fn settings(forbidden_terms: Option<&[&str]>, internal_names: Option<&[&str]>) -> Settings {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("llm.json");
    let mut value = serde_json::json!({"anthropic_api_key_ref": "op://vault/item/field"});
    if let Some(terms) = forbidden_terms {
        value["forbidden_terms"] = serde_json::json!(terms);
    }
    if let Some(names) = internal_names {
        value["internal_names"] = serde_json::json!(names);
    }
    std::fs::write(&path, value.to_string()).unwrap();
    island_llm::settings::load(&path).unwrap()
}

fn checks(audience: Audience) -> Checks {
    let settings = settings(
        Some(&["司令塔", "Formation"]),
        Some(&["secret-repo", "example-org"]),
    );
    prepare(&settings, audience).unwrap()
}

fn output(summary: &str, items: &[(&str, &[u64])]) -> serde_json::Value {
    serde_json::json!({
        "summary": summary,
        "items": items
            .iter()
            .map(|(text, sources)| serde_json::json!({"text": text, "sources": sources}))
            .collect::<Vec<_>>(),
    })
}

fn rejected(value: &serde_json::Value, audience: Audience) -> String {
    check_output(value, 3, &checks(audience)).unwrap_err()
}

#[test]
fn a_dictionary_term_is_rejected_for_either_audience() {
    let value = output("版を整えた。", &[("formation の窓を減らした", &[1])]);
    for audience in [Audience::Private, Audience::Public] {
        assert_eq!(
            rejected(&value, audience),
            "output_rejected:items.0.text.term"
        );
    }
}

#[test]
fn internal_names_are_rejected_only_for_public_and_only_as_words() {
    let named = output("Secret-Repo の検査を足した。", &[("検査を足した", &[1])]);
    assert_eq!(
        rejected(&named, Audience::Public),
        "output_rejected:summary.internal_name"
    );
    assert!(check_output(&named, 3, &checks(Audience::Private)).is_ok());

    let longer = output(
        "secret-repository の検査を足した。",
        &[("検査を足した", &[1])],
    );
    assert!(check_output(&longer, 3, &checks(Audience::Public)).is_ok());
}

#[test]
fn internal_name_inflections_are_rejected_without_matching_other_words() {
    for text in [
        "Secret-Repos",
        "Secret-Repoes",
        "Secret-Repo's",
        "Secret-Repos'",
        "Secret-Repo’s",
    ] {
        let value = output(text, &[("Updated checks.", &[1])]);
        assert_eq!(
            rejected(&value, Audience::Public),
            "output_rejected:summary.internal_name",
            "{text}"
        );
        assert!(
            check_output(&value, 3, &checks(Audience::Private)).is_ok(),
            "{text}"
        );
        let item = output("Updated checks.", &[(text, &[1])]);
        assert_eq!(
            rejected(&item, Audience::Public),
            "output_rejected:items.0.text.internal_name",
            "{text}"
        );
    }
    for text in [
        "secret-repository",
        "secret-reposition",
        "secret-repossession",
        "my-secret-repos",
        "secret-repos_x",
        "secret-repoesque",
    ] {
        let value = output(text, &[("Updated checks.", &[1])]);
        assert!(
            check_output(&value, 3, &checks(Audience::Public)).is_ok(),
            "{text}"
        );
    }
}

#[test]
fn urls_html_tokens_and_long_digits_are_rejected() {
    for (text, kind) in [
        ("詳細は https://example.com を参照", "url"),
        ("詳細は www.example.com を参照", "url"),
        ("<b>強調</b>した", "html"),
        ("鍵 sk-ant-abc を替えた", "token"),
        ("鍵 ghp_abc を替えた", "token"),
        ("password: abc を替えた", "token"),
        ("番号 1234567890 を直した", "digits"),
        ("番号 １２３４５６７８９０ を直した", "digits"),
        ("番号 12345６７８９０ を直した", "digits"),
        ("</div> を消した", "html"),
        ("<!-- 注 --> を消した", "html"),
        ("一行目\n二行目", "format"),
    ] {
        let value = output("版を整えた。", &[("検査を足した", &[1]), (text, &[2])]);
        assert_eq!(
            rejected(&value, Audience::Private),
            format!("output_rejected:items.1.text.{kind}"),
            "{text}"
        );
    }
}

#[test]
fn sources_must_be_present_and_within_the_input_numbers() {
    for (sources, kind) in [
        (serde_json::json!([]), "too_small"),
        (serde_json::json!([0]), "out_of_range"),
        (serde_json::json!([4]), "out_of_range"),
        (serde_json::json!([1, "2"]), "invalid_type"),
        (serde_json::json!(null), "invalid_type"),
    ] {
        let value = serde_json::json!({
            "summary": "版を整えた。",
            "items": [{"text": "検査を足した", "sources": sources}],
        });
        assert_eq!(
            rejected(&value, Audience::Private),
            format!("output_rejected:items.0.sources.{kind}"),
            "{sources}"
        );
    }
    let missing =
        serde_json::json!({"summary": "版を整えた。", "items": [{"text": "検査を足した"}]});
    assert_eq!(
        rejected(&missing, Audience::Private),
        "output_rejected:items.0.sources.required"
    );
}

#[test]
fn a_malformed_shape_is_rejected_with_its_location() {
    for (value, code) in [
        (serde_json::json!([]), "output_rejected:root.invalid_type"),
        (
            serde_json::json!({"summary": "版を整えた。", "items": [], "extra": 1}),
            "output_rejected:root.unrecognized_keys",
        ),
        (
            serde_json::json!({"items": [{"text": "検査を足した", "sources": [1]}]}),
            "output_rejected:summary.required",
        ),
        (
            serde_json::json!({"summary": "", "items": [{"text": "検査を足した", "sources": [1]}]}),
            "output_rejected:summary.too_small",
        ),
        (
            serde_json::json!({"summary": "版を整えた。", "items": {}}),
            "output_rejected:items.invalid_type",
        ),
        (
            serde_json::json!({"summary": "版を整えた。", "items": []}),
            "output_rejected:items.too_small",
        ),
        (
            serde_json::json!({"summary": "版を整えた。", "items": [{"text": 1, "sources": [1]}]}),
            "output_rejected:items.0.text.invalid_type",
        ),
        (
            serde_json::json!({"summary": "版を整えた。", "items": [{"text": "検査を足した", "sources": [1], "kind": "修正"}]}),
            "output_rejected:items.0.unrecognized_keys",
        ),
    ] {
        assert_eq!(rejected(&value, Audience::Private), code, "{value}");
    }
}

#[test]
fn public_without_a_complete_dictionary_does_not_infer() {
    for (terms, names) in [
        (None, None),
        (Some(&["司令塔"][..]), None),
        (None, Some(&["secret-repo"][..])),
    ] {
        assert_eq!(
            prepare(&settings(terms, names), Audience::Public).unwrap_err(),
            DICTIONARY_MISSING
        );
    }
    let private = prepare(&settings(None, None), Audience::Private).unwrap();
    assert!(private.dictionary_missing);
    assert!(!checks(Audience::Private).dictionary_missing);
}

#[test]
fn private_keeps_its_terms_when_only_internal_names_are_missing() {
    let checks = prepare(&settings(Some(&["段取り"]), None), Audience::Private).unwrap();
    assert!(!checks.dictionary_missing);
    let value = output("段取りを変更した。", &[("表示を改善した", &[1])]);
    assert_eq!(
        check_output(&value, 1, &checks).unwrap_err(),
        "output_rejected:summary.term"
    );
}

fn extract() -> ReleaseExtract {
    ReleaseExtract {
        previous: "v0.1.0".to_owned(),
        tag: "v0.2.0".to_owned(),
        repo: "example/project".to_owned(),
        items: vec![
            "- **一つ目**".to_owned(),
            "- **二つ目**".to_owned(),
            "- **三つ目**".to_owned(),
        ],
        section: None,
        commit_links: vec![None; 3],
        language: NotesLanguage::Ja,
    }
}

#[test]
fn an_unreferenced_input_item_adds_the_compare_guidance_instead_of_falling_back() {
    let value = output(
        "版を整えた。",
        &[("検査を足した", &[1]), ("表示を直した", &[1, 2])],
    );
    let notes = check_output(&value, 3, &checks(Audience::Public)).unwrap();
    assert_eq!(
        notes.render(&extract()),
        "版を整えた。\n\n- 検査を足した\n- 表示を直した\n\nその他の変更は [v0.1.0...v0.2.0](https://github.com/example/project/compare/v0.1.0...v0.2.0) を参照してください。\n"
    );
}

#[test]
fn a_fully_referenced_output_ends_with_the_compare_line() {
    let value = output(
        "版を整えた。",
        &[("検査を足した", &[1, 3]), ("表示を直した", &[2])],
    );
    let notes = check_output(&value, 3, &checks(Audience::Public)).unwrap();
    assert_eq!(
        notes.render(&extract()),
        "版を整えた。\n\n- 検査を足した\n- 表示を直した\n\n変更範囲: [v0.1.0...v0.2.0](https://github.com/example/project/compare/v0.1.0...v0.2.0)\n"
    );
}

#[test]
fn arrows_and_comparisons_are_not_html() {
    for text in [
        "速度を 2 倍 -> 3 倍へ改善した",
        "上限を a < b の順にした",
        "9 桁 123456789 は残す",
    ] {
        let value = output("版を整えた。", &[(text, &[1])]);
        assert!(
            check_output(&value, 3, &checks(Audience::Private)).is_ok(),
            "{text}"
        );
    }
}

/// `release` が本文を入れる注釈付き tag。tag の object は commit と別の SHA になる。
fn annotate(root: &Path, tag: &str) {
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "tag",
            "-a",
            "-m",
            "本文",
            tag,
        ],
    );
    assert_ne!(
        git(root, &["rev-parse", tag]),
        git(root, &["rev-parse", &format!("{tag}^{{commit}}")])
    );
}

#[test]
fn annotated_tags_render_the_range_from_the_adjacent_annotated_tag() {
    let dir = fixture();
    annotate(dir.path(), "v0.1.0");
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-09\n- **以前の項目**\n- **先の版の項目**\n",
        "first range",
    );
    annotate(dir.path(), "v0.2.0");
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-09\n- **以前の項目**\n- **先の版の項目**\n- **今回の項目**\n",
        "second range",
    );
    annotate(dir.path(), "v0.3.0");
    let output = notes(dir.path(), "v0.3.0");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "変更範囲: [v0.2.0...v0.3.0](https://github.com/example/project/compare/v0.2.0...v0.3.0)\n\n- **今回の項目**\n"
    );
}

#[test]
fn a_lightweight_previous_tag_starts_the_range_of_an_annotated_release() {
    let dir = fixture();
    git(dir.path(), &["tag", "v0.1.0"]);
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-10\n- **移行後の項目**\n\n## 2026-09\n- **以前の項目**\n",
        "change",
    );
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    let extract = extract_release(dir.path(), &head, "v0.2.0", None).unwrap();
    assert_eq!(extract.previous, "v0.1.0");
    assert_eq!(extract.items, ["- **移行後の項目**"]);

    annotate(dir.path(), "v0.2.0");
    let output = notes(dir.path(), "v0.2.0");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), extract.render());
}

#[test]
fn an_annotated_previous_tag_is_chosen_for_an_untagged_end_commit() {
    let dir = fixture();
    annotate(dir.path(), "v0.1.0");
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-10\n- **版 commit の項目**\n\n## 2026-09\n- **以前の項目**\n",
        "version commit",
    );
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    let extract = extract_release(dir.path(), &head, "v0.2.0", None).unwrap();
    assert_eq!(extract.previous, "v0.1.0");
    assert_eq!(extract.items, ["- **版 commit の項目**"]);
}

// ---- 版の節の抽出（design §10） ----

/// example-repo の CHANGELOG の実物の形（`## X.Y.Z (YYYY-MM-DD)`、要約の段落と 1 行の箇条）。
const EXAMPLE_REPO: &str = "# Changelog

Full release notes with details on each version: [GitHub Releases](https://github.com/example-org/example-repo/releases)

## Unreleased

Future changes.

## 0.2.2 (2026-10-21)

The 0.2.1 tag was pushed without a GitHub Release because its changelog section was undated; 0.2.2 carries the same changes.

- Maintenance: **CI** runs on Linux only; a platform check workflow builds and tests on Linux and macOS before a release.
- Security: **Dependencies** update rustls-webpki to 0.103.16 and webpki-roots to 1.0.10 for RUSTSEC-2026-0381 and 0382.

## 0.2.0 (2026-10-19)

Lineup adds opt-in Kimi weekly quota.

- Feature: **Kimi weekly quota** adds a separate, default-off managed account query.
";

/// packages/cli の CHANGELOG の実物の形（Keep a Changelog の `## [X.Y.Z] - YYYY-MM-DD`、`###` の小見出し）。
const KEEP_A_CHANGELOG: &str = "# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

## [0.6.1] - 2026-10-21

The `v0.6.0` tag has no Release: its Test run failed and its Release workflow did not start.

### Fixed

- Install Bun in the Test and Release workflows. `cargo test` reads a generated `.env` back
  with Bun's dotenv parser, so the Test run for 0.6.0 failed on both runners.
- Pass `TEST_SHELL` to `tests/resolve.sh` as an absolute path in the workflows and the README.

## [0.6.0] - 2026-10-21

### Added

- Publish the tools from the public `fujimogn/develop-tools` repository.

Versions before 0.6.0 were not published.
";

#[test]
fn a_dated_parenthesised_section_is_extracted_without_its_heading() {
    let section = extract_changelog_section(EXAMPLE_REPO, "0.2.2").unwrap();
    assert_eq!(
        section,
        ChangelogSection {
            body: "The 0.2.1 tag was pushed without a GitHub Release because its changelog section was undated; 0.2.2 carries the same changes.\n\n- Maintenance: **CI** runs on Linux only; a platform check workflow builds and tests on Linux and macOS before a release.\n- Security: **Dependencies** update rustls-webpki to 0.103.16 and webpki-roots to 1.0.10 for RUSTSEC-2026-0381 and 0382.".to_owned(),
            items: vec![
                "- Maintenance: **CI** runs on Linux only; a platform check workflow builds and tests on Linux and macOS before a release.".to_owned(),
                "- Security: **Dependencies** update rustls-webpki to 0.103.16 and webpki-roots to 1.0.10 for RUSTSEC-2026-0381 and 0382.".to_owned(),
            ],
        }
    );
    // 最後の節はファイルの終わりで閉じ、末尾の空行を落とす。
    let last = extract_changelog_section(EXAMPLE_REPO, "0.2.0").unwrap();
    assert_eq!(
        last.body,
        "Lineup adds opt-in Kimi weekly quota.\n\n- Feature: **Kimi weekly quota** adds a separate, default-off managed account query."
    );
}

#[test]
fn a_keep_a_changelog_section_keeps_subheadings_and_joins_continuation_lines() {
    let section = extract_changelog_section(KEEP_A_CHANGELOG, "0.6.1").unwrap();
    assert!(
        section.body.starts_with("The `v0.6.0` tag has no Release"),
        "{}",
        section.body
    );
    assert!(
        section.body.contains("### Fixed\n\n- Install Bun"),
        "{}",
        section.body
    );
    assert!(
        section.body.ends_with("in the workflows and the README."),
        "{}",
        section.body
    );
    assert!(!section.body.contains("## [0.6.0]"), "{}", section.body);
    assert_eq!(
        section.items,
        vec![
            "- Install Bun in the Test and Release workflows. `cargo test` reads a generated `.env` back with Bun's dotenv parser, so the Test run for 0.6.0 failed on both runners.".to_owned(),
            "- Pass `TEST_SHELL` to `tests/resolve.sh` as an absolute path in the workflows and the README.".to_owned(),
        ]
    );
    let older = extract_changelog_section(KEEP_A_CHANGELOG, "0.6.0").unwrap();
    assert_eq!(
        older.body,
        "### Added\n\n- Publish the tools from the public `fujimogn/develop-tools` repository.\n\nVersions before 0.6.0 were not published."
    );
}

#[test]
fn a_version_is_matched_exactly_and_headings_inside_fences_are_ignored() {
    let changelog = "## 10.2.2 (2026-10-21)\n\n- wrong\n\n```md\n## 0.2.2 (2026-10-20)\n```\n\n## 0.2.2 (2026-10-21)\n\n- right\n\n```md\n## 0.2.1 (2026-10-20)\n```\n";
    let section = extract_changelog_section(changelog, "0.2.2").unwrap();
    assert_eq!(section.body, "- right\n\n```md\n## 0.2.1 (2026-10-20)\n```");
    assert_eq!(section.items, vec!["- right".to_owned()]);
}

#[test]
fn section_failures_have_fixed_reason_codes() {
    let code = |changelog: &str| {
        extract_changelog_section(changelog, "1.2.3")
            .unwrap_err()
            .code()
    };
    // 無い: 版の見出しが無い・日付が無い・日付の形が違う・Unreleased だけ。
    for changelog in [
        "# Changelog\n\n## 1.2.2 (2026-10-12)\n\n- older\n",
        "## 1.2.3\n\n- undated\n",
        "## [1.2.3]\n\n- undated\n",
        "## 1.2.3 (2026/10/13)\n\n- slash\n",
        "## [1.2.3] 2026-10-13\n\n- no dash\n",
        "## Unreleased\n\n- future\n",
        "",
    ] {
        assert_eq!(code(changelog), "section_missing", "{changelog:?}");
    }
    // 重複: 同じ形の 2 つ・2 形式の混在。
    for changelog in [
        "## 1.2.3 (2026-10-13)\n\n- a\n\n## 1.2.3 (2026-10-14)\n\n- b\n",
        "## 1.2.3 (2026-10-13)\n\n- a\n\n## [1.2.3] - 2026-10-13\n\n- b\n",
    ] {
        assert_eq!(code(changelog), "section_duplicate", "{changelog:?}");
    }
    // 空: 見出しだけ・空白の行だけで次の節。
    for changelog in [
        "## 1.2.3 (2026-10-13)\n",
        "## [1.2.3] - 2026-10-13\n\n   \n\n## [1.2.2] - 2026-10-12\n\n- older\n",
    ] {
        assert_eq!(code(changelog), "section_empty", "{changelog:?}");
    }
}

#[test]
fn an_undated_heading_is_named_in_the_missing_message() {
    let error = extract_changelog_section("## 1.2.3\n\n- undated\n", "1.2.3").unwrap_err();
    assert!(error.to_string().contains("日付"), "{error}");
}

#[test]
fn a_section_extract_renders_the_compare_line_and_the_section_body() {
    let extract = ReleaseExtract {
        section: Some("Summary.\n\n- Fix: **A** fixed.".to_owned()),
        ..extract()
    };
    assert_eq!(
        extract.render(),
        "変更範囲: [v0.1.0...v0.2.0](https://github.com/example/project/compare/v0.1.0...v0.2.0)\n\nSummary.\n\n- Fix: **A** fixed.\n"
    );
}

/// 宣言を読んで作る本文。`v0.1.0` の後に宣言と CHANGELOG を commit し、`v0.2.0` の本文にする。
fn declared_render(notes: &str, changelog: &str) -> String {
    let dir = fixture();
    let root = dir.path();
    git(root, &["tag", "v0.1.0"]);
    std::fs::create_dir_all(root.join(".config")).unwrap();
    std::fs::write(
        root.join(".config/develop-tools.json"),
        format!("{{\"release\": {{\"notes\": {notes}}}}}\n"),
    )
    .unwrap();
    git(root, &["add", ".config/develop-tools.json"]);
    commit(root, changelog, "notes");
    let declared = declared_notes(root).unwrap().unwrap();
    let prepared = prepare_notes(root, declared, "v0.2.0").unwrap();
    prepared.extract(root, "HEAD").unwrap().render()
}

#[test]
fn the_compare_heading_follows_the_declared_language() {
    let section = "# Changelog\n\n## 0.2.0 (2026-10-23)\n\n- Fixed **A**.\n";
    let additions = "# Changelog\n\n## 2026-09\n- **以前の項目**\n- **New item**\n";
    let link = "[v0.1.0...v0.2.0](https://github.com/example/project/compare/v0.1.0...v0.2.0)";
    for (notes, changelog, expected) in [
        (
            r#"{"mode":"changelog-section","language":"en"}"#,
            section,
            format!("Compare: {link}\n\n- Fixed **A**.\n"),
        ),
        (
            r#"{"mode":"changelog-additions","language":"en"}"#,
            additions,
            format!("Compare: {link}\n\n- **New item**\n"),
        ),
        (
            r#"{"mode":"changelog-section","language":"ja"}"#,
            section,
            format!("変更範囲: {link}\n\n- Fixed **A**.\n"),
        ),
        (
            r#"{"mode":"changelog-section"}"#,
            section,
            format!("変更範囲: {link}\n\n- Fixed **A**.\n"),
        ),
    ] {
        assert_eq!(declared_render(notes, changelog), expected, "{notes}");
    }
}

#[test]
fn rewritten_and_ambiguous_items_are_listed_on_stderr_instead_of_the_release_body() {
    let dir = fixture();
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-10\n- **old title**: long body\n\n- **unchanged**\n",
        "base",
    );
    git(dir.path(), &["tag", "v0.1.0"]);
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-10\n- **renamed**: short\n- **ambiguous new**\n\n- **unchanged**\n- **pure addition**\n",
        "rewrite and add",
    );
    git(dir.path(), &["tag", "v0.2.0"]);
    let output = notes(dir.path(), "v0.2.0");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "変更範囲: [v0.1.0...v0.2.0](https://github.com/example/project/compare/v0.1.0...v0.2.0)\n\n- **pure addition**\n"
    );
    let warning = String::from_utf8(output.stderr).unwrap();
    assert!(warning.contains("警告:"));
    assert!(warning.contains("tag 注釈に補ってください"));
    assert!(warning.contains("- **renamed**: short\n- **ambiguous new**\n"));
    assert!(!warning.contains("- **pure addition**"));
}

#[test]
fn english_ai_output_uses_english_compare_guidance() {
    let extract = ReleaseExtract {
        language: NotesLanguage::En,
        ..extract()
    };
    let value = output(
        "Selection improved.",
        &[("Choose an item from the list.", &[1])],
    );
    let notes = check_output(&value, 3, &checks(Audience::Public)).unwrap();
    assert_eq!(
        notes.render(&extract),
        format!(
            "Selection improved.\n\n- Choose an item from the list.\n\nSee {} for other changes.\n",
            extract.compare_link()
        )
    );
    let complete = check_output(&value, 1, &checks(Audience::Public)).unwrap();
    assert!(
        complete
            .render(&extract)
            .ends_with(&format!("Compare: {}\n", extract.compare_link()))
    );
}

#[test]
fn english_public_output_keeps_the_same_checks() {
    let checks = checks(Audience::Public);
    for (value, code) in [
        (
            output("A FORMATION change.", &[("Selection improved.", &[1])]),
            "summary.term",
        ),
        (
            output("Changes.", &[("Updated Secret-Repo checks.", &[1])]),
            "items.0.text.internal_name",
        ),
        (
            output("Changes.", &[("Selection improved.", &[2])]),
            "items.0.sources.out_of_range",
        ),
        (
            serde_json::json!({"summary":"Changes.","items":[{"text":"Selection improved."}]}),
            "items.0.sources.required",
        ),
        (
            output("Changes.", &[("Read https://example.com", &[1])]),
            "items.0.text.url",
        ),
    ] {
        assert_eq!(
            check_output(&value, 1, &checks).unwrap_err(),
            format!("output_rejected:{code}")
        );
    }
}

#[test]
fn compact_versions_select_only_this_release_and_unreleased_items() {
    use develop_release::release_notes::extract_changelog_additions;
    let current = "- **今回** 2026-11-05/v0.2.0: 本文";
    let pending = "- **未公開** 2026-11-05 ⏳: 本文の /v0.1.0 は残す";
    let ordinary = "- **日付（2026-11-05）**: 本文の /v0.1.0 は残す";
    let old_style = "- **旧刻印（2026-11-05、v0.1.0 で prod 反映）**: 本文";
    let diff = format!(
        "@@ -1 +1,6 @@\n ## 2026-11\n+- **前の版** 2026-11-04/v0.1.0: 除く\n+{current}\n+{pending}\n+{ordinary}\n+{old_style}\n"
    );
    assert_eq!(
        extract_changelog_additions(&diff, "v0.2.0"),
        [current, pending, ordinary]
    );
}

#[test]
fn scrub_preserves_body_metadata_and_obsolete_parenthesized_stamps() {
    assert_eq!(
        scrub_for_ai("- **題** 2026-11-05 ⏳: 本文の 2026-11-04/v0.1.0 と ⏳"),
        "- **題**: 本文の 2026-11-04/v0.1.0 と ⏳"
    );
    for item in [
        "- **題（2026-11-05、未リリース）**: 本文",
        "- **題（2026-11-05、v0.1.0 で prod 反映）**: 本文",
    ] {
        assert_eq!(scrub_for_ai(item), "- **題**: 本文");
    }
}

fn fixed_manifest(root: &Path, path: &str, notes: &str) {
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, format!(r#"{{"release":{{"notes":{notes}}}}}"#)).unwrap();
    git(root, &["add", path]);
}

fn fixed_fixture() -> (tempfile::TempDir, String) {
    let dir = fixture();
    let root = dir.path();
    annotate(root, "v0.1.0");
    fixed_manifest(
        root,
        ".config/develop-tools.json",
        r#"{"mode":"changelog-additions","language":"en"}"#,
    );
    commit(
        root,
        "# Changelog\n\n## 2026-09\n- **以前の項目**\n- **固定終点**\n",
        "end",
    );
    let end = git(root, &["rev-parse", "HEAD"]);
    (dir, end)
}

#[test]
fn fixed_inputs_keep_both_oids_and_the_endpoint_declaration_after_refs_move() {
    use develop_release::release_notes::FixedNotes;
    let (dir, end) = fixed_fixture();
    let root = dir.path();
    let fixed = FixedNotes::resolve(root, "v0.1.0", &end, "v0.3.0", None).unwrap();
    fixed_manifest(
        root,
        ".config/develop-tools.json",
        r#"{"mode":"changelog-section","language":"ja"}"#,
    );
    commit(
        root,
        "# Changelog\n\n## 0.3.0 (2026-11-06)\n\n- later\n",
        "later",
    );
    let resolved_later = FixedNotes::resolve(root, "v0.1.0", &end, "v0.3.0", None).unwrap();
    assert_eq!(
        resolved_later.extract().unwrap().language,
        NotesLanguage::En
    );
    assert_eq!(resolved_later.extract().unwrap().items, ["- **固定終点**"]);
    git(root, &["tag", "-f", "v0.1.0"]);
    std::fs::write(
        root.join(".config/develop-tools.json"),
        "invalid dirty JSON",
    )
    .unwrap();
    std::fs::write(root.join("CHANGELOG.md"), "dirty").unwrap();
    let result = fixed.extract().unwrap();
    assert_eq!(result.previous, "v0.1.0");
    assert_eq!(result.tag, "v0.3.0");
    assert_eq!(result.language, NotesLanguage::En);
    assert_eq!(result.items, ["- **固定終点**"]);
    // 後続 HEAD と dirty file だけでなく、解決時にも終点の宣言を使う。
    assert_eq!(
        FixedNotes::resolve(root, "v0.1.0", &end, "v0.3.0", None).unwrap_err(),
        "from は to の祖先である必要があります"
    );
}

#[test]
fn fixed_inputs_reject_equal_divergent_and_non_commit_ranges() {
    use develop_release::release_notes::FixedNotes;
    let (dir, end) = fixed_fixture();
    let root = dir.path();
    git(root, &["tag", "v0.2.0"]);
    assert!(
        FixedNotes::resolve(root, "v0.2.0", &end, "v0.3.0", None)
            .unwrap_err()
            .contains("同一")
    );
    for (from, to, tag) in [
        ("v0.2.0", end.as_str(), "v0.3.0"),
        ("v0.1.0", end.as_str(), "v0.1.0"),
        ("v9.9.9", end.as_str(), "v0.3.0"),
        ("HEAD", end.as_str(), "v0.3.0"),
        ("--help", end.as_str(), "v0.3.0"),
        ("v0.1.0", "HEAD", "v0.3.0"),
        ("v0.1.0", &end[..12], "v0.3.0"),
        ("v0.1.0", "--help", "v0.3.0"),
        ("v0.1.0", end.as_str(), "invalid"),
    ] {
        assert!(
            FixedNotes::resolve(root, from, to, tag, None).is_err(),
            "{from} {to} {tag}"
        );
    }
    let base = git(root, &["rev-parse", "v0.1.0^{commit}"]);
    git(root, &["checkout", "--detach", &base]);
    commit(root, "# Changelog\n\n## 2026-09\n- **別枝**\n", "divergent");
    git(root, &["tag", "v0.1.1"]);
    assert!(FixedNotes::resolve(root, "v0.1.1", &end, "v0.3.0", None).is_err());
    let tree = git(root, &["rev-parse", "HEAD^{tree}"]);
    assert!(FixedNotes::resolve(root, "v0.1.0", &tree, "v0.3.0", None).is_err());
}

#[test]
fn fixed_inputs_locate_manifest_only_in_endpoint_tree() {
    use develop_release::release_notes::FixedNotes;
    use develop_tools_core::develop_tools_manifest::{LEGACY_PATH, PATH};
    for (paths, notes, reason) in [
        (vec![], r#"{"mode":"changelog-additions"}"#, "release.notes"),
        (
            vec![LEGACY_PATH],
            r#"{"mode":"changelog-additions"}"#,
            "manifest_legacy",
        ),
        (
            vec![PATH, LEGACY_PATH],
            r#"{"mode":"changelog-additions"}"#,
            "manifest_duplicated",
        ),
        (vec![PATH], r#"{"mode":"unknown"}"#, "release.notes.mode"),
    ] {
        let dir = fixture();
        let root = dir.path();
        git(root, &["tag", "v0.1.0"]);
        for path in paths {
            fixed_manifest(root, path, notes);
        }
        commit(root, "# Changelog\n\n## 2026-09\n- **追加**\n", "end");
        let end = git(root, &["rev-parse", "HEAD"]);
        fixed_manifest(root, PATH, r#"{"mode":"changelog-additions"}"#);
        let error = FixedNotes::resolve(root, "v0.1.0", &end, "v0.2.0", None).unwrap_err();
        assert!(error.contains(reason), "{error}");
    }
}

#[test]
fn fixed_inputs_read_section_and_ai_fallback_from_endpoint() {
    use develop_release::release_notes::{FixedNotes, NotesPlan};
    for notes in [
        r#"{"mode":"changelog-section","language":"en"}"#,
        r#"{"mode":"ai","fallback":"changelog-section","audience":"public","language":"en"}"#,
    ] {
        let dir = fixture();
        let root = dir.path();
        git(root, &["tag", "v0.1.0"]);
        fixed_manifest(root, ".config/develop-tools.json", notes);
        commit(
            root,
            "# Changelog\n\n## 0.2.0 (2026-11-06)\n\nEndpoint summary.\n\n- Fixed.\n",
            "end",
        );
        let end = git(root, &["rev-parse", "HEAD"]);
        commit(root, "later", "later");
        let fixed =
            FixedNotes::resolve(root, "v0.1.0", &end, "v0.2.0", Some("other/project")).unwrap();
        assert_eq!(
            fixed.extract().unwrap().render(),
            "Compare: [v0.1.0...v0.2.0](https://github.com/other/project/compare/v0.1.0...v0.2.0)\n\nEndpoint summary.\n\n- Fixed.\n"
        );
        assert_eq!(
            matches!(fixed.plan(), NotesPlan::Ai { .. }),
            notes.contains("audience")
        );
    }
}

#[test]
fn legacy_tag_cli_keeps_grammar_and_exit_codes_without_reading_declarations() {
    let (dir, _) = fixed_fixture();
    let root = dir.path();
    annotate(root, "v0.2.0");
    std::fs::write(root.join(".config/develop-tools.json"), "invalid JSON").unwrap();
    let expected = notes(root, "v0.2.0");
    assert_eq!(expected.status.code(), Some(0));
    for args in [
        vec!["v0.2.0", "--repo", "example/project"],
        vec!["--repo", "example/project", "v0.2.0"],
    ] {
        let output = notes_args(root, &args);
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(output.stdout, expected.stdout);
    }
    for args in [
        vec![],
        vec!["--repo"],
        vec!["--unknown"],
        vec!["v0.2.0", "extra"],
        vec!["v0.2.0", "--repo", "bad"],
        vec!["v9.9.9"],
        vec!["render"],
    ] {
        let output = notes_args(root, &args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(!output.stderr.is_empty(), "{args:?}");
    }
    assert_eq!(notes_args(root, &["--help"]).status.code(), Some(0));
}

#[test]
fn fixed_inputs_accept_full_sha256_commit_oids() {
    use develop_release::release_notes::FixedNotes;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "--object-format=sha256"]);
    commit(root, "# Changelog\n\n## 2026-11\n- **以前**\n", "initial");
    annotate(root, "v0.1.0");
    fixed_manifest(
        root,
        ".config/develop-tools.json",
        r#"{"mode":"changelog-additions"}"#,
    );
    commit(
        root,
        "# Changelog\n\n## 2026-11\n- **以前**\n- **追加**\n",
        "end",
    );
    let end = git(root, &["rev-parse", "HEAD"]);
    assert_eq!(end.len(), 64);
    let fixed =
        FixedNotes::resolve(root, "v0.1.0", &end, "v0.2.0", Some("example/project")).unwrap();
    assert_eq!(fixed.extract().unwrap().items, ["- **追加**"]);
    assert!(
        FixedNotes::resolve(
            root,
            "v0.1.0",
            &end[..40],
            "v0.2.0",
            Some("example/project")
        )
        .is_err()
    );
}

#[test]
fn fixed_inputs_use_repo_root_from_subdirectories_even_with_decoy_files() {
    use develop_release::release_notes::FixedNotes;
    for with_decoy in [false, true] {
        let (dir, mut end) = fixed_fixture();
        let root = dir.path();
        let sub = root.join("sub");
        std::fs::create_dir(&sub).unwrap();
        if with_decoy {
            fixed_manifest(
                root,
                "sub/.config/develop-tools.json",
                r#"{"mode":"changelog-section","language":"ja"}"#,
            );
            std::fs::write(sub.join("CHANGELOG.md"), "decoy").unwrap();
            git(root, &["add", "sub/CHANGELOG.md"]);
            commit(
                root,
                "# Changelog\n\n## 2026-09\n- **以前の項目**\n- **固定終点**\n",
                "decoys",
            );
            end = git(root, &["rev-parse", "HEAD"]);
        }
        let fixed = FixedNotes::resolve(&sub, "v0.1.0", &end, "v0.2.0", None).unwrap();
        let extract = fixed.extract().unwrap();
        assert_eq!(extract.language, NotesLanguage::En);
        assert_eq!(extract.items, ["- **固定終点**"]);
    }
}

#[test]
fn legacy_stamp_fixture_selects_items_without_writing_changelog() {
    let dir = fixture();
    let root = dir.path();
    git(root, &["tag", "v0.1.0"]);
    let current = "- **今回（2026-11-05、v0.2.0 で prod 反映）**: 本文";
    let pending = "- **予定（未リリース）**: 本文";
    let dated = "- **日付付き（2026-11-05、未リリース）**: 本文";
    let ordinary = "- **印なし**: 本文の（2026-11-05、v0.1.0 で prod 反映）は残す";
    let middle = "- **説明（未リリース）の続き**: 本文";
    let changelog = format!(
        "# Changelog\n\n## 2026-11\n{current}\n{pending}\n{dated}\n{ordinary}\n{middle}\n- **旧版（2026-11-05、v0.1.0 で prod 反映）**: 除く\n  - **入れ子（未リリース）** 2026-11-05/v0.1.0: 説明\n```md\n- **fence（未リリース）** 2026-11-05/v0.1.0: 例\n```\n\n## 2026-09\n- **以前の項目**\n"
    );
    commit(root, &changelog, "legacy fixture");
    let before = std::fs::read(root.join("CHANGELOG.md")).unwrap();
    let head = git(root, &["rev-parse", "HEAD"]);
    let result = extract_release(root, &head, "v0.2.0", None).unwrap();
    assert_eq!(result.items, [current, pending, dated, ordinary, middle]);
    assert_eq!(std::fs::read(root.join("CHANGELOG.md")).unwrap(), before);
    assert_eq!(scrub_for_ai(current), "- **今回**: 本文");
    assert_eq!(scrub_for_ai(pending), "- **予定**: 本文");
    assert_eq!(scrub_for_ai(dated), "- **日付付き**: 本文");
    assert_eq!(scrub_for_ai(ordinary), ordinary);
    assert_eq!(scrub_for_ai(middle), middle);
    let nested = "  - **入れ子（未リリース）**: 本文";
    // scrub の既存の空白正規化だけを許す。
    assert_eq!(scrub_for_ai(nested), nested.trim_start());
}

#[test]
fn legacy_and_compact_stamps_are_an_extraction_error() {
    for stamp in ["2026-11-05 ⏳", "2026-11-05/v0.2.0", "2026-11-05/v0.1.0"] {
        let dir = fixture();
        let root = dir.path();
        git(root, &["tag", "v0.1.0"]);
        commit(
            root,
            &format!(
                "# Changelog\n\n## 2026-11\n- **曖昧（未リリース）** {stamp}: 本文\n\n## 2026-09\n- **以前の項目**\n"
            ),
            "ambiguous fixture",
        );
        let before = std::fs::read(root.join("CHANGELOG.md")).unwrap();
        let head = git(root, &["rev-parse", "HEAD"]);
        assert!(
            extract_release(root, &head, "v0.2.0", None)
                .unwrap_err()
                .contains("changelog_stamp_ambiguous")
        );
        git(root, &["tag", "v0.2.0"]);
        let output = notes(root, "v0.2.0");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(std::fs::read(root.join("CHANGELOG.md")).unwrap(), before);
    }
}

#[test]
fn release_rejects_ambiguous_stamps_before_writing_version_commit() {
    assert_release_rejects_ambiguous_stamps(false);
}

#[test]
fn release_rejects_pending_ambiguous_stamps_already_present_in_previous_tag() {
    assert_release_rejects_ambiguous_stamps(true);
}

fn assert_release_rejects_ambiguous_stamps(present_in_previous_tag: bool) {
    let dir = fixture();
    let root = dir.path();
    git(root, &["branch", "-M", "main"]);
    git(root, &["config", "user.name", "Test"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["config", "tag.gpgSign", "false"]);
    if present_in_previous_tag {
        commit(
            root,
            "# Changelog\n\n## 2026-11\n- **曖昧（未リリース）** 2026-11-05 ⏳: 本文\n\n## 2026-09\n- **以前の項目**\n",
            "pending stamp before previous tag",
        );
    }
    git(root, &["tag", "v0.1.0"]);
    git(root, &["checkout", "-b", "develop"]);
    std::fs::create_dir(root.join(".config")).unwrap();
    std::fs::write(
        root.join(".config/develop-tools.json"),
        r#"{
        "integrationBranch":"develop",
        "release":{
            "versionFiles":[{"path":"package.json","kind":"package-json"}],
            "changelogStamp":true,
            "notes":{"mode":"changelog-additions"}
        }
    }"#,
    )
    .unwrap();
    std::fs::write(root.join("package.json"), "{\"version\":\"0.1.0\"}\n").unwrap();
    git(root, &["add", ".config/develop-tools.json", "package.json"]);
    commit(
        root,
        "# Changelog\n\n## 2026-11\n- **曖昧（未リリース）** 2026-11-05 ⏳: 本文\n\n## 2026-09\n- **以前の項目**\n",
        "ambiguous release input",
    );
    let before_head = git(root, &["rev-parse", "HEAD"]);
    let before_main = git(root, &["rev-parse", "main"]);
    let before_changelog = std::fs::read(root.join("CHANGELOG.md")).unwrap();
    let before_package = std::fs::read(root.join("package.json")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_release"))
        .args(["patch", "--no-ai"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("changelog_stamp_ambiguous"));
    assert_eq!(git(root, &["rev-parse", "HEAD"]), before_head);
    assert_eq!(git(root, &["rev-parse", "main"]), before_main);
    assert_eq!(
        std::fs::read(root.join("CHANGELOG.md")).unwrap(),
        before_changelog
    );
    assert_eq!(
        std::fs::read(root.join("package.json")).unwrap(),
        before_package
    );
    assert!(git(root, &["tag", "--list", "v0.1.1"]).is_empty());
    assert!(git(root, &["status", "--porcelain"]).is_empty());
}

fn render_args(end: &str) -> Vec<&str> {
    vec!["render", "--from", "v0.1.0", "--to", end, "--tag", "v0.3.0"]
}

fn diagnostic(output: &std::process::Output) -> serde_json::Value {
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    serde_json::from_str(stderr.lines().last().expect("終端 JSON")).unwrap()
}

fn snapshot(root: &Path) -> (String, String, Vec<u8>, Vec<u8>, Vec<u8>) {
    (
        git(root, &["show-ref"]),
        git(root, &["rev-parse", "HEAD"]),
        std::fs::read(root.join(".git/index")).unwrap(),
        std::fs::read(root.join("CHANGELOG.md")).unwrap(),
        std::fs::read(root.join("package.json")).unwrap(),
    )
}

#[test]
fn render_reads_fixed_commit_in_detached_dirty_tree_without_mutation() {
    let (dir, end) = fixed_fixture();
    let root = dir.path();
    git(root, &["checkout", "--detach"]);
    std::fs::write(root.join("CHANGELOG.md"), "secret dirty body").unwrap();
    std::fs::write(root.join("package.json"), "dirty version file").unwrap();
    std::fs::write(
        root.join(".config/develop-tools.json"),
        "invalid dirty JSON",
    )
    .unwrap();
    git(root, &["add", "package.json"]);
    let before = snapshot(root);
    let output = notes_args(root, &render_args(&end));
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout.clone()).unwrap(),
        "Compare: [v0.1.0...v0.3.0](https://github.com/example/project/compare/v0.1.0...v0.3.0)\n\n- **固定終点**\n"
    );
    assert_eq!(
        diagnostic(&output),
        serde_json::json!({"source":"changelog","reason_code":null})
    );
    assert_eq!(snapshot(root), before);
    let mut bad = render_args("HEAD");
    bad.extend(["--repo", "secret-invalid-repo"]);
    let output = notes_args(root, &bad);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(diagnostic(&output)["source"], "error");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("secret"));
    assert_eq!(snapshot(root), before);
}

#[test]
fn render_rejects_duplicate_unknown_missing_and_positional_options() {
    let (dir, end) = fixed_fixture();
    for extra in [
        vec!["--from", "v0.1.0"],
        vec!["--to", &end],
        vec!["--tag", "v0.4.0"],
        vec!["--repo", "a/b", "--repo", "a/b"],
        vec!["--no-ai", "--no-ai"],
        vec!["--unknown"],
        vec!["extra"],
        vec!["--repo"],
        vec!["--repo", "--no-ai"],
    ] {
        let mut args = render_args(&end);
        args.extend(extra);
        let output = notes_args(dir.path(), &args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty());
        assert_eq!(
            diagnostic(&output),
            serde_json::json!({"source":"error","reason_code":"arguments_invalid"})
        );
    }
    for args in [
        vec!["render"],
        vec!["render", "--from"],
        vec!["render", "--to", "--tag", "v0.3.0"],
    ] {
        let output = notes_args(dir.path(), &args);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(diagnostic(&output)["reason_code"], "arguments_invalid");
    }
}

#[test]
fn render_reports_fallback_and_no_ai_without_exposing_body_in_stderr() {
    let (dir, _) = fixed_fixture();
    let root = dir.path();
    fixed_manifest(
        root,
        ".config/develop-tools.json",
        r#"{"mode":"ai","fallback":"changelog-additions","audience":"private"}"#,
    );
    commit(
        root,
        "# Changelog\n\n## 2026-09\n- **以前の項目**\n- **固定終点**\n",
        "ai declaration",
    );
    let end = git(root, &["rev-parse", "HEAD"]);
    let home = tempfile::tempdir().unwrap();
    for no_ai in [false, true] {
        let mut args = render_args(&end);
        if no_ai {
            args.push("--no-ai");
        }
        let output = Command::new(env!("CARGO_BIN_EXE_release-notes"))
            .args(args)
            .current_dir(root)
            .env("HOME", home.path())
            .env_remove("OP_SERVICE_ACCOUNT_TOKEN")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(String::from_utf8_lossy(&output.stdout).contains("固定終点"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("固定終点"));
        assert_eq!(
            diagnostic(&output),
            if no_ai {
                serde_json::json!({"source":"no_ai","reason_code":null})
            } else {
                serde_json::json!({"source":"fallback","reason_code":"key_missing"})
            }
        );
    }
}

#[test]
fn render_redacts_edited_items_and_structural_errors() {
    let (dir, _) = fixed_fixture();
    let root = dir.path();
    commit(
        root,
        "# Changelog\n\n## 2026-09\n- **secret edited body**\n",
        "edit",
    );
    let end = git(root, &["rev-parse", "HEAD"]);
    let output = notes_args(root, &render_args(&end));
    assert_eq!(output.status.code(), Some(0));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("secret"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("edited_items_excluded"));
    assert_eq!(diagnostic(&output)["source"], "changelog");
    commit(
        root,
        "# Changelog\n\n## 2026-09\n- **以前の項目**\n- **secret（未リリース）** 2026-11-05 ⏳: 本文\n",
        "ambiguous",
    );
    let end = git(root, &["rev-parse", "HEAD"]);
    let output = notes_args(root, &render_args(&end));
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        diagnostic(&output)["reason_code"],
        "changelog_stamp_ambiguous"
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("secret"));
}

#[test]
fn render_does_not_take_a_structural_reason_code_from_an_input_value() {
    let (dir, end) = fixed_fixture();
    for repo in ["manifest_legacy", "manifest_duplicated", "section_missing"] {
        let mut args = render_args(&end);
        args.extend(["--repo", repo]);
        let output = notes_args(dir.path(), &args);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(diagnostic(&output)["reason_code"], "fixed_input_invalid");
    }
}

#[test]
fn render_section_keeps_structural_codes_and_explicit_repo() {
    let (dir, _) = fixed_fixture();
    let root = dir.path();
    fixed_manifest(
        root,
        ".config/develop-tools.json",
        r#"{"mode":"changelog-section","language":"en"}"#,
    );
    commit(root, "# Changelog\n", "missing section");
    let end = git(root, &["rev-parse", "HEAD"]);
    let output = notes_args(root, &render_args(&end));
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(diagnostic(&output)["reason_code"], "section_missing");

    commit(
        root,
        "# Changelog\n\n## 0.3.0 (2026-11-06)\n\nFixed section.\n",
        "section",
    );
    let end = git(root, &["rev-parse", "HEAD"]);
    let mut args = render_args(&end);
    args.extend(["--repo", "other/project"]);
    let output = notes_args(root, &args);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout.clone()).unwrap(),
        "Compare: [v0.1.0...v0.3.0](https://github.com/other/project/compare/v0.1.0...v0.3.0)\n\nFixed section.\n"
    );
    assert_eq!(diagnostic(&output)["source"], "changelog");
}

#[test]
fn help_publishes_both_entry_points_and_render_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let output = notes_args(dir.path(), &["--help"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let help = String::from_utf8(output.stdout).unwrap();
    for contract in [
        "release-notes <vX.Y.Z> [--repo owner/name]",
        "release-notes render --from <vX.Y.Z> --to <full-commit-SHA> --tag <vA.B.C> [--repo owner/name] [--no-ai]",
        "stderr の最後の 1 行が JSON",
        "API の最大 1 回の送信、usage 台帳と fallback JSONL への追記だけ",
        "版 file・CHANGELOG・index・commit・branch・tag・remote・GitHub Release・prod は変更しません",
        "AI 成功では常に dictionary_missing boolean（private 辞書欠落時 true、それ以外 false）",
        "help は release-notes --help で表示（render --help は文法エラー）",
        "source=ai/changelog/no_ai/fallback",
        "source=error",
        "旧刻印の他版項目を除外",
    ] {
        assert!(help.contains(contract), "help に公開契約がない: {contract}");
    }
    let output = notes_args(dir.path(), &["render", "--help"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        diagnostic(&output),
        serde_json::json!({"source":"error","reason_code":"arguments_invalid"})
    );
}

#[test]
fn render_contract_accepts_reordered_options_and_rejects_missing_required_options() {
    let (dir, end) = fixed_fixture();
    let output = notes_args(
        dir.path(),
        &[
            "render",
            "--no-ai",
            "--tag",
            "v0.3.0",
            "--repo",
            "example/project",
            "--to",
            &end,
            "--from",
            "v0.1.0",
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        diagnostic(&output),
        serde_json::json!({"source":"changelog","reason_code":null})
    );
    let body = std::str::from_utf8(&output.stdout).unwrap();
    assert!(body.ends_with('\n'));
    assert!(body.contains("https://github.com/example/project/compare/v0.1.0...v0.3.0"));
    for missing in ["--from", "--to", "--tag"] {
        let args = render_args(&end);
        let mut reduced = vec!["render"];
        for pair in args[1..].chunks_exact(2) {
            if pair[0] != missing {
                reduced.extend_from_slice(pair);
            }
        }
        let output = notes_args(dir.path(), &reduced);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(
            diagnostic(&output),
            serde_json::json!({"source":"error","reason_code":"arguments_invalid"})
        );
    }
}

fn commit_link(sha: &str) -> String {
    format!("[{sha}](https://github.com/example/project/commit/{sha})")
}

fn linked_extract() -> (tempfile::TempDir, ReleaseExtract, Vec<String>) {
    let dir = fixture();
    let root = dir.path();
    git(root, &["tag", "v0.1.0"]);
    let first = git(root, &["rev-parse", "HEAD"]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-qm",
            "implementation",
        ],
    );
    let second = git(root, &["rev-parse", "HEAD"]);
    let links = vec![
        commit_link(&first[..7]),
        commit_link(&second[..8]),
        commit_link(&first[..12]),
    ];
    commit(
        root,
        &format!(
            "# Changelog\n\n## 2026-09\n- **以前の項目**\n- **一つ目** {}\n- **linkなし**\n- **三つ目** {}\n- **同じcommit** {}\n- **未参照** {}\n",
            links[0],
            links[1],
            links[2],
            commit_link(&second)
        ),
        "record",
    );
    let extracted = extract_release(root, "HEAD", "v0.2.0", None).unwrap();
    (dir, extracted, links)
}

#[test]
fn commit_links_follow_checked_sources_and_deduplicate_full_oids() {
    let (_dir, extract, links) = linked_extract();
    let value = output(
        "改善しました。",
        &[
            ("結合", &[4, 3, 1, 1, 2]),
            ("再参照", &[4]),
            ("linkなし", &[2]),
        ],
    );
    let notes = check_output(&value, 5, &Checks::default()).unwrap();
    let rendered = notes.render(&extract);
    assert!(
        rendered.contains(&format!("- 結合 {} {}\n", links[0], links[1])),
        "{rendered}"
    );
    assert!(
        rendered.contains(&format!("- 再参照 {}\n", links[0])),
        "{rendered}"
    );
    assert!(rendered.contains("- linkなし\n"));
    assert!(!rendered.contains(&links[2]));
    assert_eq!(rendered.lines().next(), Some("改善しました。"));
    assert!(rendered.contains("その他の変更は"));
    let last = extract.items[4]
        .split_once("](")
        .unwrap()
        .0
        .rsplit('[')
        .next()
        .unwrap();
    assert!(!rendered.contains(&commit_link(last)));
}

#[test]
fn commit_link_suffix_is_removed_before_scrubbing_without_losing_numbers() {
    for sha in [
        "1234567",
        "abcdefa",
        "1234567890123456789012345678901234567890",
        "abcdefabcdefabcdefabcdefabcdefabcdefabcd",
    ] {
        assert_eq!(
            scrub_for_ai(&format!("- **成果** {}\n  継続", commit_link(sha))),
            "- **成果**\n 継続"
        );
        assert_eq!(scrub_for_ai(&format!("- {}", commit_link(sha))), "-");
    }
    assert_eq!(
        numbered_ai_input(&[String::new(), "本文".to_owned()]),
        "[1] \n[2] 本文\n"
    );
}

#[test]
fn commit_links_require_origin_even_with_repo_override_only_when_present() {
    let (dir, extract, _) = linked_extract();
    let root = dir.path();
    git(root, &["remote", "remove", "origin"]);
    let error = extract_release(root, "HEAD", "v0.2.0", Some("example/project")).unwrap_err();
    assert!(error.contains("commit_link"), "{error}");
    commit(
        root,
        "# Changelog\n\n## 2026-09\n- **以前の項目**\n- **linkなし**\n",
        "without links",
    );
    assert!(extract_release(root, "HEAD", "v0.2.0", Some(&extract.repo)).is_ok());
}

#[test]
fn commit_links_outside_fixed_endpoint_are_extraction_errors() {
    use develop_release::release_notes::FixedNotes;
    let dir = fixture();
    let root = dir.path();
    git(root, &["tag", "v0.1.0"]);
    let base = git(root, &["rev-parse", "HEAD"]);
    commit(root, "other branch", "outside");
    let outside = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "--detach", &base]);
    fixed_manifest(
        root,
        ".config/develop-tools.json",
        r#"{"mode":"ai","fallback":"changelog-additions","audience":"private"}"#,
    );
    commit(
        root,
        &format!(
            "# Changelog\n\n## 2026-09\n- **以前の項目**\n- **範囲外** {}\n",
            commit_link(&outside[..7])
        ),
        "endpoint",
    );
    let end = git(root, &["rev-parse", "HEAD"]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "merge",
            "--no-edit",
            "-s",
            "ours",
            &outside,
        ],
    );
    let fixed = FixedNotes::resolve(root, "v0.1.0", &end, "v0.2.0", None).unwrap();
    let error = fixed.extract().unwrap_err();
    assert!(error.contains("commit_link"), "{error}");
}

struct LinkTransport {
    value: serde_json::Value,
    input: std::cell::RefCell<Option<serde_json::Value>>,
}

impl island_llm::transport::Transport for LinkTransport {
    fn send(
        &self,
        request: &island_llm::transport::ApiRequest<'_>,
    ) -> Result<island_llm::transport::ApiResponse, island_llm::transport::TransportError> {
        *self.input.borrow_mut() = Some(request.body.clone());
        Ok(island_llm::transport::ApiResponse {
            status: 200,
            request_id: Some("fixture".to_owned()),
            body: serde_json::json!({"content": [{"type": "text", "text": self.value.to_string()}], "usage": {"input_tokens": 10, "output_tokens": 3}}).to_string(),
        })
    }
}

#[test]
fn commit_links_survive_modes_and_mock_ai_without_entering_ai_input() {
    use develop_release::release_notes::{
        AiRuntime, BodySource, DeclaredNotes, NotesPlan, release_body,
    };
    use develop_tools_core::develop_tools_manifest::NotesSource;
    use std::os::unix::fs::PermissionsExt;
    let (dir, additions, links) = linked_extract();
    let root = dir.path();
    let env = tempfile::tempdir().unwrap();
    let settings = env.path().join("settings.json");
    std::fs::write(&settings, serde_json::json!({"anthropic_api_key_ref": "op://test/key/value", "forbidden_terms": [], "internal_names": [], "usage_ledger": env.path().join("usage.jsonl")}).to_string()).unwrap();
    let op = env.path().join("fake-op");
    std::fs::write(&op, "#!/bin/sh\nprintf 'test-api-key'\n").unwrap();
    std::fs::set_permissions(&op, std::fs::Permissions::from_mode(0o755)).unwrap();
    let plan = NotesPlan::Ai {
        audience: Audience::Private,
    };
    let transport = LinkTransport {
        value: output("改善しました。", &[("結合", &[3, 1, 2])]),
        input: Default::default(),
    };
    let runtime = AiRuntime {
        home: env.path().join("home"),
        op_program: op,
        op_service_account_token: Some("fixture".to_owned()),
        transport: &transport,
    };
    commit(
        root,
        &format!(
            "# Changelog\n\n## 0.2.0 (2026-11-06)\n\nSummary.\n\n- **一つ目** {}\n- **linkなし**\n- **三つ目** {}\n",
            links[0], links[1]
        ),
        "section",
    );
    let section = prepare_notes(
        root,
        DeclaredNotes {
            plan,
            source: NotesSource::ChangelogSection,
            language: NotesLanguage::Ja,
        },
        "v0.2.0",
    )
    .unwrap()
    .extract(root, "HEAD")
    .unwrap();
    for extract in [additions, section] {
        for (mode, no_ai, path, expected) in [
            (
                NotesPlan::Changelog,
                false,
                settings.clone(),
                BodySource::Changelog,
            ),
            (plan, true, settings.clone(), BodySource::NoAi),
            (
                plan,
                false,
                env.path().join("missing"),
                BodySource::Fallback("settings_missing".to_owned()),
            ),
        ] {
            let body = release_body(&extract, mode, no_ai, &path, &runtime);
            assert_eq!(body.text, extract.render());
            assert!(body.text.contains(&links[0]));
            // fallback の code は既存の設定 reader の契約に委ねる。
            if matches!(expected, BodySource::Fallback(_)) {
                assert!(matches!(body.source, BodySource::Fallback(_)));
            } else {
                assert_eq!(body.source, expected);
            }
        }
        let body = release_body(&extract, plan, false, &settings, &runtime);
        assert!(
            matches!(body.source, BodySource::Ai { .. }),
            "{:?}",
            body.source
        );
        assert!(
            body.text
                .contains(&format!("- 結合 {} {}\n", links[0], links[1])),
            "{}",
            body.text
        );
        let input = transport.input.borrow();
        let input = input.as_ref().unwrap()["messages"][0]["content"]
            .as_str()
            .unwrap();
        assert!(
            input.contains("[1] - **一つ目**\n[2] - **linkなし**\n[3] - **三つ目**\n"),
            "{input}"
        );
        for link in &links {
            let sha = link.split(']').next().unwrap().trim_start_matches('[');
            assert!(!input.contains(sha), "{input}");
        }
        assert!(!input.contains("https://"));
    }
    let malicious = LinkTransport {
        value: output(
            "改善",
            &[("https://github.com/example/project/commit/abcdefa", &[1])],
        ),
        input: Default::default(),
    };
    let runtime = AiRuntime {
        transport: &malicious,
        ..runtime
    };
    let (_dir, extract, _) = linked_extract();
    let body = release_body(&extract, plan, false, &settings, &runtime);
    assert_eq!(
        body.source,
        BodySource::Fallback("output_rejected:items.0.text.url".to_owned())
    );
    assert_eq!(body.text, extract.render());
}

#[test]
fn commit_links_in_sections_are_checked_before_writing_a_version_commit() {
    use develop_release::release_notes::{DeclaredNotes, NotesPlan};
    use develop_tools_core::develop_tools_manifest::NotesSource;
    for first_release in [true, false] {
        let dir = fixture();
        let root = dir.path();
        if !first_release {
            git(root, &["tag", "v0.1.0"]);
        }
        let oid = git(root, &["rev-parse", "HEAD"]);
        let link = commit_link(&oid[..7]).replace("example/project", "other/project");
        commit(
            root,
            &format!("# Changelog\n\n## 0.2.0 (2026-11-06)\n\n- **成果** {link}\n"),
            "section",
        );
        let before = git(root, &["rev-parse", "HEAD"]);
        let error = prepare_notes(
            root,
            DeclaredNotes {
                plan: NotesPlan::Changelog,
                source: NotesSource::ChangelogSection,
                language: NotesLanguage::Ja,
            },
            "v0.2.0",
        )
        .unwrap_err();
        assert!(error.contains("commit_link"), "{error}");
        assert_eq!(git(root, &["rev-parse", "HEAD"]), before);
        assert_eq!(git(root, &["status", "--porcelain"]), "");
    }
}

#[test]
fn commit_links_match_origin_repository_ignoring_ascii_case() {
    let (dir, expected, links) = linked_extract();
    git(
        dir.path(),
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:Example/PROJECT.git",
        ],
    );
    let extract = extract_release(dir.path(), "HEAD", "v0.2.0", None).unwrap();
    assert_eq!(extract.items, expected.items);
    let notes = check_output(
        &output("改善", &[("成果", &[1])]),
        extract.items.len(),
        &Checks::default(),
    )
    .unwrap();
    assert!(notes.render(&extract).contains(&links[0]));
}

#[test]
fn commit_links_reject_sha256_repositories_without_rejecting_old_entries() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "--object-format=sha256"]);
    git(
        root,
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:example/project.git",
        ],
    );
    commit(root, "# Changelog\n\n## 2026-11\n- **以前**\n", "initial");
    git(root, &["tag", "v0.1.0"]);
    let oid = git(root, &["rev-parse", "HEAD"]);
    commit(
        root,
        &format!(
            "# Changelog\n\n## 2026-11\n- **以前**\n- **成果** {}\n",
            commit_link(&oid[..7])
        ),
        "record",
    );
    let error = extract_release(root, "HEAD", "v0.2.0", None).unwrap_err();
    assert!(error.contains("commit_link"), "{error}");
}
