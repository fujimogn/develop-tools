//! 7 実装の食い違い（調査記録 `docs/reviews/2026-09-30-island-common-tool-candidates.md` §1.4）を、
//! 代表的な入力で島の基準（design §5.1）に固定する。
//!
//! 各 case の doc comment に、どの実装と結果が割れるかを書く（実装は repo 名ではなく記号
//! 「移設元 A〜F」で書く）。移行で旧テストと並走させたとき
//! （task 4.6）の差分は、ここに載っている規則差として説明できなければならない。

use std::path::Path;
use std::sync::Once;

use island_docs::check::{Finding, Input, evaluate};
use island_docs::constants::{ScopeTable, initialize_scopes};
use island_docs::links::{Problem, Resolution, resolve};
use island_docs::markdown::{github_slug, heading_anchors, link_targets};

fn targets(text: &str) -> Vec<String> {
    link_targets(text).into_iter().map(|t| t.target).collect()
}

/// 内蔵の表は無いので、crate の中の fixture の表を一度だけ入れてから判定する。
fn use_fixture_scopes() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let table = ScopeTable::from_copy(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            Some(include_bytes!("../src/scope-fixture.json")),
        )
        .expect("fixture の scope 表");
        initialize_scopes(table).expect("一度だけ入れる");
    });
}

fn problems(files: &[(&str, &str)]) -> Vec<(String, usize, Problem)> {
    use_fixture_scopes();
    let mut input = Input::default();
    for (path, text) in files {
        input.snapshot.add_markdown(path, text.as_bytes().to_vec());
    }
    evaluate(&input)
        .findings
        .into_iter()
        .filter_map(|finding| match finding {
            Finding::Link(link) => Some((link.target, link.line, link.problem)),
            // 必須 file・索引などの検査項目は各 module のテストが見る。
            Finding::Rule(_) => None,
            other => panic!("link 以外の finding: {other}"),
        })
        .collect()
}

// ---- anchor slug ----

/// 移設元 A `_markdown-local-links.ts:49-58` は U+FE0F を落とさず `️-注意` になる。
#[test]
fn variation_selector_is_dropped_before_filtering() {
    assert_eq!(github_slug("⚠️ 注意"), "-注意");
}

/// 移設元 B `agent_docs.rs:549-581` は `_` と結合文字（`\p{M}`）を消す。
#[test]
fn underscore_and_combining_marks_are_kept() {
    assert_eq!(github_slug("snake_case"), "snake_case");
    // `が` を基底文字 + 濁点（U+3099）で書いたもの。
    assert_eq!(github_slug("か\u{3099}"), "か\u{3099}");
}

/// 移設元 A は trim して空白の並びを 1 つの `-` にまとめる。GitHub は空白を 1 文字ずつ `-` にする。
/// 移設元 A の運用文書の 1 つはこの差で切れたまま通っていた（試走で検出）。
#[test]
fn every_whitespace_becomes_a_dash_without_trimming() {
    assert_eq!(github_slug("A — B"), "a--b");
    assert_eq!(
        github_slug("B. API トークンで接続する（Claude Code / 自動化向け）"),
        "b-api-トークンで接続するclaude-code--自動化向け"
    );
}

/// 重複した見出しは GitHub と同じく `-1`、`-2` を付ける。
#[test]
fn duplicate_headings_are_numbered() {
    let anchors = heading_anchors("# 概要\n## 概要\n### 概要\n");
    for slug in ["概要", "概要-1", "概要-2"] {
        assert!(anchors.contains(slug), "{slug}");
    }
}

/// HTML タグ・code span・link 記法は文字だけ残す。`<a name>` も anchor に数える（terraform-docs の出力）。
#[test]
fn markup_is_stripped_and_html_anchors_count() {
    assert_eq!(
        github_slug("`cfg` と [doc](x.md) <sup>1</sup>"),
        "cfg-と-doc-1"
    );
    assert!(heading_anchors("## <a name=\"input_zone\"></a> zone\n").contains("input_zone"));
}

// ---- fence ----

/// 移設元 C `validate-adrs.ts:21` は `~~~` を fence と認識しない。
#[test]
fn tilde_fence_hides_links_and_headings() {
    let text = "~~~\n# 見出し\n[x](missing.md)\n~~~\n";
    assert!(targets(text).is_empty());
    assert!(heading_anchors(text).is_empty());
}

/// 移設元 D `repository.test.ts:87` の `/```[\s\S]*?```/g` は、4 本の fence の中の ``` で閉じてしまう。
#[test]
fn longer_fence_is_not_closed_by_a_shorter_marker() {
    let text = "````md\n```\n[x](missing.md)\n```\n````\n[y](real.md)\n";
    assert_eq!(targets(text), ["real.md"]);
}

/// 移設元 B `agent_docs.rs:737` は fence を除かない本文から anchor を作る。link と anchor は同じ fence 判定を使う。
#[test]
fn heading_inside_fence_is_not_an_anchor() {
    let found = problems(&[("a.md", "```sh\n# comment\n```\n[x](#comment)\n")]);
    assert_eq!(found, [("#comment".to_owned(), 4, Problem::MissingAnchor)]);
}

/// インデント 4 は fence ではない（CommonMark）。
#[test]
fn four_space_indent_does_not_open_a_fence() {
    assert_eq!(targets("    ```\n[x](a.md)\n"), ["a.md"]);
}

// ---- 外部 scheme ----

/// 移設元 D は `startsWith("http")` だけを外部とみなし、`mailto:` を path として探し、`http-notes.md` を素通しする。
#[test]
fn schemes_are_external_and_http_prefixed_files_are_local() {
    assert_eq!(
        resolve("a.md", "mailto:a@example.com"),
        Resolution::External
    );
    assert_eq!(resolve("a.md", "vscode://file/x"), Resolution::External);
    let found = problems(&[("a.md", "[n](http-notes.md) [m](mailto:a@example.com)\n")]);
    assert_eq!(
        found,
        [("http-notes.md".to_owned(), 1, Problem::MissingPath)]
    );
}

// ---- title・`<>`・reference 形式 ----

/// 移設元 E、移設元 F、移設元 D は title まで path に含める。全対応は移設元 A だけだった。
#[test]
fn titles_angle_brackets_and_reference_definitions() {
    let text = "[a](a.md \"題\") [b](<with space.md>)\n[r]: ./ref.md 'x'\n";
    assert_eq!(targets(text), ["a.md", "with space.md", "./ref.md"]);
}

/// footnote 定義（`[^1]: 本文`）は reference 定義ではない（移設元 A の reference 正規表現は本文の先頭語を宛先に取る）。
#[test]
fn footnote_definitions_are_not_links() {
    assert!(targets("[^1]: 注の本文\n").is_empty());
}

/// inline code の中は link ではない（移設元 F `docs-context:105-116` は除かない）。
#[test]
fn links_inside_code_spans_are_ignored() {
    assert!(targets("`[x](missing.md)` と ``[y](m.md)``\n").is_empty());
}

// ---- repo 外・絶対 path ----

/// repo の外へ出る相対 path と端末固有の絶対 path を同じ検査で拒否する。
#[test]
fn neighbour_repository_and_absolute_paths_are_rejected() {
    let found = problems(&[(
        "docs/a.md",
        "[n](../../other-repo/README.md) [abs](/etc/x.md) [h](/home/u/x.md) [m](/Users/u/x.md) [t](~/x.md)\n",
    )]);
    assert_eq!(
        found,
        [
            (
                "../../other-repo/README.md".to_owned(),
                1,
                Problem::OutsideRepository
            ),
            ("/etc/x.md".to_owned(), 1, Problem::AbsolutePath),
            ("/home/u/x.md".to_owned(), 1, Problem::HostPath),
            ("/Users/u/x.md".to_owned(), 1, Problem::HostPath),
            ("~/x.md".to_owned(), 1, Problem::HostPath),
        ]
    );
}

// ---- 存在確認の基準 ----

/// 移設元 A 以外は FS を見るので、untracked の file が手元でだけ通る。島は tracked（index）だけを見る。
/// snapshot に無い path は、手元の FS にあっても無いものとして扱う。
#[test]
fn only_tracked_paths_exist() {
    let found = problems(&[("a.md", "[u](untracked.md) [d](docs/)\n")]);
    assert_eq!(
        found,
        [
            ("untracked.md".to_owned(), 1, Problem::MissingPath),
            ("docs/".to_owned(), 1, Problem::MissingPath),
        ]
    );
}

/// tracked の dir は存在する宛先。percent-encoding は decode してから探す。
#[test]
fn tracked_directories_and_encoded_names_resolve() {
    let found = problems(&[
        ("docs/sub/x.md", "# x\n"),
        (
            "a.md",
            "[d](docs/sub/) [e](docs/sub/x.md#x) [s](my%20file.md)\n",
        ),
        ("my file.md", "# s\n"),
    ]);
    assert!(found.is_empty(), "{found:?}");
}

/// terraform-docs の `#input\_name` は backslash escape を外してから照合する（試走で移設元 B の 29 件）。
#[test]
fn escaped_fragments_match_html_anchors() {
    let found = problems(&[(
        "README.md",
        "[z](#input\\_zone\\_id)\n\n## <a name=\"input_zone_id\"></a> zone\n",
    )]);
    assert!(found.is_empty(), "{found:?}");
}
