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
    Command::new(env!("CARGO_BIN_EXE_release-notes"))
        .arg(tag)
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
fn dates_and_prod_stamps_are_dropped() {
    assert_eq!(
        scrub_for_ai("- **版の刻印を直した（2026-10-05、v0.5.3 で prod 反映）**: 本文"),
        "- **版の刻印を直した**: 本文"
    );
    assert_eq!(
        scrub_for_ai("- **未リリースの項目（2026-10-05、未リリース）**: 本文"),
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
        "# Changelog\n\n## 2026-10\n- **版 commit の項目**\n",
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
        "# Changelog\n\n## 2026-10\n- **移行後の項目**\n",
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
        "# Changelog\n\n## 2026-10\n- **版 commit の項目**\n",
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
    std::fs::write(
        root.join(".develop-tools.json"),
        format!("{{\"release\": {{\"notes\": {notes}}}}}\n"),
    )
    .unwrap();
    git(root, &["add", ".develop-tools.json"]);
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
