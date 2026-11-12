use develop_release::release_notes::extract_changelog_section;

#[test]
fn monthly_version_section_preserves_summary_and_seven_items_and_stops_at_next_section() {
    let body = "Restores the quota and makes Git headings reliable.\n\n\
- **Quota (2026-11-09/⏳)**: Fix: recognizes weekly periods.\n\
- **Heading (2026-11-09/⏳)**: Fix: keeps publishing.\n\
- **Cache (2026-11-09/⏳)**: Fix: scopes invalidation.\n\
- **Stop (2026-11-09/⏳)**: Fix: shares the deadline.\n\
- **Dependencies (2026-11-09/⏳)**: Maintenance: refreshes crates.\n\
- **Tests (2026-11-09/⏳)**: Maintenance: stabilizes ordering.\n\
- **History (2026-11-09/⏳)**: Maintenance: preserves contents.";
    let changelog = format!(
        "# Changelog\n\n## 2026-11\n\n### 0.3.3 (2026-11-09)\n\n{body}\n\n### 0.3.2 (2026-11-08)\n\nPrevious release.\n- Previous item\n\n## 2026-10\n\nOlder month.\n"
    );
    let section = extract_changelog_section(&changelog, "0.3.3").unwrap();
    assert_eq!(section.body, body);
    assert_eq!(section.items.len(), 7);
    assert_eq!(
        section.items[0],
        "- **Quota (2026-11-09/⏳)**: Fix: recognizes weekly periods."
    );
    assert_eq!(
        section.items[6],
        "- **History (2026-11-09/⏳)**: Maintenance: preserves contents."
    );
}

#[test]
fn monthly_version_section_rejects_a_heading_date_outside_its_month() {
    let error = extract_changelog_section(
        "## 2026-10\n\n### 0.3.3 (2026-11-09)\n\n- Fix: wrong month\n",
        "0.3.3",
    )
    .unwrap_err();
    assert_eq!(error.code(), "section_month_mismatch");
}

#[test]
fn section_render_removes_stamp_metadata_but_keeps_summary_supplement_body_and_fences() {
    use develop_release::release_notes::prepare_notes;
    use develop_tools_core::develop_tools_manifest::NotesLanguage;
    let dir = section_repo(
        "## 2026-11\n### 0.3.3 (2026-11-09)\n\nSummary.\n\n- **題（2026-11-09/⏳、補足）**: 本文の /⏳ は説明。\n\n```md\n- **例（2026-11-09/⏳）**: fence は保持\n```",
        true,
    );
    let mut declared = section_declaration();
    declared.language = NotesLanguage::Ja;
    let extract = prepare_notes(dir.path(), declared, "v0.3.3")
        .unwrap()
        .extract(dir.path(), "HEAD")
        .unwrap();
    assert_eq!(
        extract.render(),
        "変更範囲: [v0.3.2...v0.3.3](https://github.com/example/project/compare/v0.3.2...v0.3.3)\n\nSummary.\n\n- **題（補足）**: 本文の /⏳ は説明。\n\n```md\n- **例（2026-11-09/⏳）**: fence は保持\n```\n"
    );
}

#[test]
fn monthly_version_sections_reject_duplicate_versions_and_ignore_fenced_headings() {
    let duplicate =
        "## 2026-11\n### 0.3.3 (2026-11-09)\n- first\n### 0.3.3 (2026-11-10)\n- second\n";
    assert_eq!(
        extract_changelog_section(duplicate, "0.3.3")
            .unwrap_err()
            .code(),
        "section_duplicate"
    );
    let fenced = "## 2026-11\n### 0.3.3 (2026-11-09)\nSummary.\n```md\n### 0.3.3 (2026-11-09)\n```\n- right\n## 2026-10\n- older\n";
    let section = extract_changelog_section(fenced, "0.3.3").unwrap();
    assert!(section.body.ends_with("- right"));
    assert_eq!(section.items, ["- right"]);
}

#[test]
fn english_section_render_removes_pending_and_release_links_from_the_title() {
    use develop_release::release_notes::prepare_notes;
    let dir = section_repo(
        "## 2026-11\n### 0.3.3 (2026-11-09)\n\nSummary.\n\n- **Pending (2026-11-09/⏳, fast pane)**: Fix: body.\n- **Released (2026-11-09/[v0.3.3](https://github.com/example/project/releases/tag/v0.3.3))**: Maintenance: more.",
        true,
    );
    let extract = prepare_notes(dir.path(), section_declaration(), "v0.3.3")
        .unwrap()
        .extract(dir.path(), "HEAD")
        .unwrap();
    assert_eq!(
        extract.render(),
        "Compare: [v0.3.2...v0.3.3](https://github.com/example/project/compare/v0.3.2...v0.3.3)\n\nSummary.\n\n- **Pending (fast pane)**: Fix: body.\n- **Released**: Maintenance: more.\n"
    );
}

fn git(root: &std::path::Path, args: &[&str]) {
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
}

fn section_repo(body: &str, previous: bool) -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("develop-meta-release-section-")
        .tempdir()
        .unwrap();
    let root = dir.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.name", "Test"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "maintenance.auto", "false"]);
    git(
        root,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/project.git",
        ],
    );
    std::fs::write(root.join("CHANGELOG.md"), "# Changelog\n").unwrap();
    git(root, &["add", "CHANGELOG.md"]);
    git(root, &["commit", "-qm", "initial"]);
    if previous {
        git(root, &["tag", "v0.3.2"]);
    }
    std::fs::write(root.join("CHANGELOG.md"), body).unwrap();
    git(root, &["add", "CHANGELOG.md"]);
    git(root, &["commit", "-qm", "notes"]);
    dir
}

fn section_declaration() -> develop_release::release_notes::DeclaredNotes {
    use develop_release::release_notes::{DeclaredNotes, NotesPlan};
    use develop_tools_core::develop_tools_manifest::{NotesLanguage, NotesSource};
    DeclaredNotes {
        plan: NotesPlan::Ai {
            audience: island_llm::settings::Audience::Public,
        },
        source: NotesSource::ChangelogSection,
        language: NotesLanguage::En,
    }
}

struct NoRequests;
impl island_llm::transport::Transport for NoRequests {
    fn send(
        &self,
        _: &island_llm::transport::ApiRequest<'_>,
    ) -> Result<island_llm::transport::ApiResponse, island_llm::transport::TransportError> {
        panic!("fixture must not send an API request");
    }
}

#[test]
fn declared_monthly_section_normalizes_both_ai_material_and_no_ai_or_failure_fallback() {
    use develop_release::release_notes::{AiRuntime, BodySource, prepare_notes, release_body};
    let dir = section_repo(
        "## 2026-11\n### 0.3.3 (2026-11-09)\n\nSummary.\n\n- **Pending (2026-11-09/⏳)**: Fix: body.\n",
        true,
    );
    let root = dir.path();
    let prepared = prepare_notes(root, section_declaration(), "v0.3.3").unwrap();
    let extract = prepared.extract(root, "HEAD").unwrap();
    assert_eq!(extract.items, ["- **Pending**: Fix: body."]);
    let runtime = AiRuntime {
        home: root.to_owned(),
        op_program: root.join("no-op"),
        op_service_account_token: None,
        transport: &NoRequests,
    };
    let settings = root.join("missing-settings");
    let direct = release_body(&extract, prepared.plan(), true, &settings, &runtime);
    let fallback = release_body(&extract, prepared.plan(), false, &settings, &runtime);
    let expected = "Compare: [v0.3.2...v0.3.3](https://github.com/example/project/compare/v0.3.2...v0.3.3)\n\nSummary.\n\n- **Pending**: Fix: body.\n";
    assert_eq!(direct.text, expected);
    assert_eq!(fallback.text, expected);
    assert_eq!(direct.source, BodySource::NoAi);
    assert!(matches!(fallback.source, BodySource::Fallback(_)));
}

#[test]
fn a_monthly_section_rejects_items_stamped_for_another_version_or_release_url() {
    use develop_release::release_notes::prepare_notes;
    for item in [
        "- **Other version (2026-11-09/v0.3.2)**: body.",
        "- **Other repo (2026-11-09/[v0.3.3](https://github.com/other/project/releases/tag/v0.3.3))**: body.",
        "- **Wrong tag (2026-11-09/[v0.3.3](https://github.com/example/project/releases/tag/v0.3.2))**: body.",
    ] {
        let dir = section_repo(
            &format!("## 2026-11\n### 0.3.3 (2026-11-09)\n\n{item}\n"),
            true,
        );
        let error = prepare_notes(dir.path(), section_declaration(), "v0.3.3").unwrap_err();
        assert!(error.contains("changelog_stamp_invalid"), "{error}");
    }
}

#[test]
fn the_first_release_also_renders_a_normalized_monthly_section_without_ai() {
    use develop_release::release_notes::{AiRuntime, BodySource, prepare_notes};
    let dir = section_repo(
        "## 2026-11\n### 0.3.3 (2026-11-09)\n\nInitial summary.\n\n- **First (2026-11-09/⏳)**: body.\n",
        false,
    );
    let root = dir.path();
    let prepared = prepare_notes(root, section_declaration(), "v0.3.3").unwrap();
    let runtime = AiRuntime {
        home: root.to_owned(),
        op_program: root.join("no-op"),
        op_service_account_token: None,
        transport: &NoRequests,
    };
    let body = prepared
        .body(
            root,
            "HEAD",
            false,
            &root.join("missing-settings"),
            &runtime,
        )
        .unwrap();
    assert_eq!(body.text, "Initial summary.\n\n- **First**: body.\n");
    assert_eq!(body.source, BodySource::FirstRelease);
}

#[test]
fn stamp_shaped_supplements_are_preserved_once_in_items_and_release_body() {
    use develop_release::release_notes::prepare_notes;
    use develop_tools_core::develop_tools_manifest::NotesLanguage;
    for (language, input, expected) in [
        (
            NotesLanguage::Ja,
            "- **題（2026-11-09/⏳、2026-11-08/v0.3.2）**: 本文",
            "- **題（2026-11-08/v0.3.2）**: 本文",
        ),
        (
            NotesLanguage::En,
            "- **Title (2026-11-09/⏳, 2026-11-08/v0.3.2)**: Body",
            "- **Title (2026-11-08/v0.3.2)**: Body",
        ),
    ] {
        let dir = section_repo(
            &format!("## 2026-11\n### 0.3.3 (2026-11-09)\n\n{input}\n"),
            true,
        );
        let mut declared = section_declaration();
        declared.language = language;
        let extract = prepare_notes(dir.path(), declared, "v0.3.3")
            .unwrap()
            .extract(dir.path(), "HEAD")
            .unwrap();
        assert_eq!(extract.items, [expected]);
        assert!(extract.render().contains(expected), "{}", extract.render());
    }
}
