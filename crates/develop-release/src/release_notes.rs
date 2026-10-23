//! tag 間の CHANGELOG 月別項目から GitHub Release 本文を作る。

use std::path::{Path, PathBuf};

use develop_tools_core::develop_tools_manifest::{
    NotesAudience, NotesLanguage, NotesMode, NotesSource, ReleaseNotes,
};
use island_llm::settings::{self, Audience};
use island_llm::transport::Transport;
use island_llm::{Failure, Output};

use crate::ai_check;

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    git_raw(root, args).map(|text| text.trim().to_owned())
}

/// 出力を変えずに返す。file の中身を読むときは行頭の空白を落とさないためこちらを使う。
fn git_raw(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = develop_tools_core::git(root)
        .args(args)
        .output()
        .map_err(|err| format!("git を起動できません: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} に失敗しました: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|err| format!("git の出力が UTF-8 ではありません: {err}"))
}

fn release_tag(tag: &str) -> bool {
    tag.strip_prefix('v')
        .is_some_and(|version| version.parse::<crate::Version>().is_ok())
}

fn valid_repo(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let valid_part = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    matches!((parts.next(), parts.next(), parts.next()), (Some(owner), Some(name), None) if valid_part(owner) && valid_part(name))
}

fn remote_repo(root: &Path) -> Result<String, String> {
    let remote = git(root, &["remote", "get-url", "origin"])?;
    let path = remote
        .strip_prefix("git@github.com:")
        .or_else(|| remote.strip_prefix("ssh://git@github.com/"))
        .or_else(|| remote.strip_prefix("https://github.com/"))
        .ok_or_else(|| format!("origin は GitHub URL ではありません: {remote}"))?;
    let repo = path.strip_suffix(".git").unwrap_or(path);
    if !valid_repo(repo) {
        return Err(format!("origin から repo を安全に決められません: {remote}"));
    }
    Ok(repo.to_owned())
}

fn monthly_heading(line: &str) -> bool {
    let Some(month) = line.strip_prefix("## ") else {
        return false;
    };
    let bytes = month.as_bytes();
    bytes.len() == 7
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && matches!(
            &bytes[5..],
            b"01"
                | b"02"
                | b"03"
                | b"04"
                | b"05"
                | b"06"
                | b"07"
                | b"08"
                | b"09"
                | b"10"
                | b"11"
                | b"12"
        )
}

fn fence_marker(line: &str) -> Option<(&str, &str)> {
    let text = line.trim_start_matches(' ');
    if line.len() - text.len() > 3 {
        return None;
    }
    let first = text
        .bytes()
        .next()
        .filter(|byte| matches!(byte, b'`' | b'~'))?;
    let marker_len = text.bytes().take_while(|byte| *byte == first).count();
    let marker = text.get(..marker_len)?;
    if marker_len < 3 {
        return None;
    }
    Some((marker, &text[marker_len..]))
}

/// code fence の開閉の状態。`step` は fence の印の行で `true` を返し、開閉を進める。
#[derive(Default)]
struct Fence {
    open: Option<String>,
}

impl Fence {
    fn step(&mut self, line: &str) -> bool {
        let Some((marker, rest)) = fence_marker(line) else {
            return false;
        };
        match &self.open {
            None => self.open = Some(marker.to_owned()),
            Some(open)
                if marker.starts_with(&open[..1])
                    && marker.len() >= open.len()
                    && rest.trim().is_empty() =>
            {
                self.open = None
            }
            _ => {}
        }
        true
    }

    fn is_open(&self) -> bool {
        self.open.is_some()
    }
}

fn released_for_other_tag(item: &str, tag: &str) -> bool {
    let title = item
        .strip_prefix(crate::CHANGELOG_TITLE_PREFIX)
        .unwrap_or(item)
        .split(crate::CHANGELOG_TITLE_END)
        .next()
        .unwrap_or("");
    let Some(version) = crate::released_version_suffix(title) else {
        return false;
    };
    let marked = format!("v{version}");
    release_tag(&marked) && marked != tag
}

/// 全行の文脈を含む diff から、月別節に追加されたトップレベル項目だけを採る。
pub fn extract_changelog_additions(diff: &str, tag: &str) -> Vec<String> {
    let mut additions = Vec::new();
    let mut monthly = false;
    let mut fence = Fence::default();
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with('-') {
            continue;
        }
        let Some((prefix, content)) = line.split_at_checked(1) else {
            continue;
        };
        if prefix != "+" && prefix != " " {
            continue;
        }
        if fence.step(content) || fence.is_open() {
            continue;
        }
        if content.starts_with("# ") || content.starts_with("## ") {
            monthly = monthly_heading(content);
        } else if monthly
            && prefix == "+"
            && content.starts_with(crate::CHANGELOG_TITLE_PREFIX)
            && !released_for_other_tag(content, tag)
        {
            additions.push(content.to_owned());
        }
    }
    additions
}

/// 前 tag から終点の commit までの抽出。AI の入力と fallback の本文はどちらもここから作る。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseExtract {
    pub previous: String,
    pub tag: String,
    pub repo: String,
    pub items: Vec<String>,
    /// 版の節の抽出（`changelog-section`）のときの節の中身。`items` はその節の箇条。
    pub section: Option<String>,
}

impl ReleaseExtract {
    /// 前 tag から今回の tag への compare の link（`[範囲](URL)`）。
    pub fn compare_link(&self) -> String {
        let Self {
            previous,
            tag,
            repo,
            ..
        } = self;
        format!("[{previous}...{tag}](https://github.com/{repo}/compare/{previous}...{tag})")
    }

    /// 前 tag から今回の tag への compare の行。
    pub fn compare_line(&self) -> String {
        format!("変更範囲: {}", self.compare_link())
    }

    /// 抽出をそのまま並べた本文（`changelog-additions` は項目、`changelog-section` は節の中身）。
    pub fn render(&self) -> String {
        if let Some(section) = &self.section {
            format!("{}\n\n{section}\n", self.compare_line())
        } else if self.items.is_empty() {
            format!("{}\n", self.compare_line())
        } else {
            format!("{}\n\n{}\n", self.compare_line(), self.items.join("\n"))
        }
    }
}

/// 抽出の起点。`release` は版 commit を打つ前にこれを決め、書いた後に失敗しうる処理を減らす。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseBase {
    pub previous: String,
    pub repo: String,
}

/// `parent` から辿れる最も近い release tag と、origin の repo を決める。`parent` は版 commit の親。
pub fn release_base(root: &Path, parent: &str) -> Result<ReleaseBase, String> {
    resolve_base(root, parent, None)
}

fn resolve_base(root: &Path, parent: &str, repo_arg: Option<&str>) -> Result<ReleaseBase, String> {
    let repo = match repo_arg {
        Some(repo) if valid_repo(repo) => repo.to_owned(),
        Some(repo) => return Err(format!("repo は owner/name で指定してください: {repo}")),
        None => remote_repo(root)?,
    };
    let previous = git(root, &["describe", "--tags", "--abbrev=0", parent])?;
    if !release_tag(&previous) {
        return Err(format!("前 tag を安全に決定できません: {previous}"));
    }
    Ok(ReleaseBase { previous, repo })
}

/// 決めておいた起点から終点の commit までを抽出する。版 commit の後はこの diff だけを読む。
pub fn extract_from(
    root: &Path,
    base: &ReleaseBase,
    end: &str,
    tag: &str,
) -> Result<ReleaseExtract, String> {
    let ReleaseBase { previous, repo } = base;
    let diff = git(
        root,
        &[
            "diff",
            "--unified=99999",
            &format!("{previous}..{end}"),
            "--",
            "CHANGELOG.md",
        ],
    )?;
    Ok(ReleaseExtract {
        items: extract_changelog_additions(&diff, tag),
        previous: previous.clone(),
        tag: tag.to_owned(),
        repo: repo.clone(),
        section: None,
    })
}

/// 終点の commit と予定の tag 名から抽出する。tag はまだ無くてよい（`release` が版 commit から本文を作るため）。
/// 不確実な tag や remote は抽出せず失敗する。
pub fn extract_release(
    root: &Path,
    end: &str,
    tag: &str,
    repo_arg: Option<&str>,
) -> Result<ReleaseExtract, String> {
    if !release_tag(tag) {
        return Err(format!(
            "release tag は v<semver> で指定してください: {tag}"
        ));
    }
    let repo_root = git(root, &["rev-parse", "--show-toplevel"])?;
    let root = Path::new(&repo_root);
    let end = git(
        root,
        &["rev-parse", "--verify", &format!("{end}^{{commit}}")],
    )?;
    let base = resolve_base(root, &format!("{end}^"), repo_arg)?;
    extract_from(root, &base, &end, tag)
}

/// 既存の tag の本文。tag の commit を終点に `extract_release` を使う。
pub fn release_notes(root: &Path, tag: &str, repo_arg: Option<&str>) -> Result<String, String> {
    let end = format!("refs/tags/{tag}");
    extract_release(root, &end, tag, repo_arg).map(|extract| extract.render())
}

/// 版の節の中身と、AI の入力にするトップレベルの箇条。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangelogSection {
    pub body: String,
    pub items: Vec<String>,
}

/// 版の節を本文にできない理由。`code` は止めるときの stderr に出す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionError {
    /// 日付付きの見出しが無い。`undated` は日付の無い同じ版の見出しがあったか。
    Missing {
        undated: bool,
    },
    /// 読む commit に CHANGELOG.md が無い。
    NoChangelog,
    Duplicate,
    Empty,
}

impl SectionError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Missing { .. } | Self::NoChangelog => "section_missing",
            Self::Duplicate => "section_duplicate",
            Self::Empty => "section_empty",
        }
    }
}

impl std::fmt::Display for SectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            Self::Missing { undated: true } => {
                "版の見出しに日付がありません（`## X.Y.Z (YYYY-MM-DD)` か `## [X.Y.Z] - YYYY-MM-DD`）"
            }
            Self::Missing { undated: false } => "CHANGELOG.md にその版の日付付きの節がありません",
            Self::NoChangelog => "CHANGELOG.md がありません",
            Self::Duplicate => "その版の日付付きの見出しが 2 つ以上あります",
            Self::Empty => "その版の節が空です",
        };
        write!(f, "{}: {reason}", self.code())
    }
}

/// 版の見出しなら版と、`## X.Y.Z (YYYY-MM-DD)` / `## [X.Y.Z] - YYYY-MM-DD` の日付付きの形かを返す。
/// 版の後が日付の形でない見出し（`## 1.2.3`・`## [1.2.3]`・`## 1.2.3 (2026/10/13)`）は `false`。
fn version_heading(line: &str) -> Option<(&str, bool)> {
    let heading = line.strip_prefix("## ")?.trim_end();
    let (version, dated) = match heading.strip_prefix('[') {
        Some(bracketed) => {
            let (version, rest) = bracketed.split_once(']')?;
            (
                version,
                rest.strip_prefix(" - ").is_some_and(crate::is_date),
            )
        }
        None => {
            let (version, rest) = heading.split_once(' ').unwrap_or((heading, ""));
            let date = rest
                .strip_prefix('(')
                .and_then(|rest| rest.strip_suffix(')'));
            (version, date.is_some_and(crate::is_date))
        }
    };
    version.parse::<crate::Version>().ok()?;
    Some((version, dated))
}

/// 各行に、fence の印か fence の中かを付けて返す。
fn outside_fences(text: &str) -> impl Iterator<Item = (usize, &str, bool)> {
    let mut fence = Fence::default();
    text.lines()
        .enumerate()
        .map(move |(index, line)| (index, line, fence.step(line) || fence.is_open()))
}

/// CHANGELOG から `version` の版の節を取り出す。見出しは本文に含めず、`###` 以下は含める（design §10）。
pub fn extract_changelog_section(
    changelog: &str,
    version: &str,
) -> Result<ChangelogSection, SectionError> {
    let lines: Vec<&str> = changelog.lines().collect();
    let mut start = None;
    let mut end = lines.len();
    let mut undated = false;
    for (index, line, fenced) in outside_fences(changelog) {
        if fenced || !(line.starts_with("# ") || line.starts_with("## ")) {
            continue;
        }
        if start.is_some() && end == lines.len() {
            end = index;
        }
        match version_heading(line) {
            Some((found, true)) if found == version => {
                if start.is_some() {
                    return Err(SectionError::Duplicate);
                }
                start = Some(index + 1);
                end = lines.len();
            }
            Some((found, false)) if found == version => undated = true,
            _ => {}
        }
    }
    let start = start.ok_or(SectionError::Missing { undated })?;
    let body = lines[start..end]
        .iter()
        .skip_while(|line| line.trim().is_empty())
        .copied()
        .collect::<Vec<_>>();
    let trailing = body
        .iter()
        .rev()
        .take_while(|line| line.trim().is_empty())
        .count();
    let body = &body[..body.len() - trailing];
    if body.is_empty() {
        return Err(SectionError::Empty);
    }
    let body = body.join("\n");
    Ok(ChangelogSection {
        items: section_items(&body),
        body,
    })
}

/// 節のトップレベルの箇条（行頭 `- `）。字下げした後続の行は空白 1 つで同じ項目へ連結する。
fn section_items(body: &str) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    let mut open = false;
    for (_, line, fenced) in outside_fences(body) {
        if fenced {
            open = false;
        } else if line.starts_with("- ") {
            items.push(line.trim_end().to_owned());
            open = true;
        } else if open && line.starts_with([' ', '\t']) && !line.trim().is_empty() {
            let item = items.last_mut().expect("open は箇条の後だけ");
            item.push(' ');
            item.push_str(line.trim());
        } else {
            open = false;
        }
    }
    items
}

/// 落とす番号の頭文字（lessons の `L59`・internal の `D11`）。
const LESSON_ID_PREFIXES: &[u8] = b"LD";

/// link の先の長さ（閉じ括弧の手前まで）。先の中の対になった括弧と `\\` の escape を読み飛ばす。
fn link_target_len(target: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut escaped = false;
    for (index, byte) in target.bytes().enumerate() {
        match byte {
            _ if escaped => escaped = false,
            b'\\' => escaped = true,
            b'(' => depth += 1,
            b')' if depth == 0 => return Some(index),
            b')' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// `[文言](先)` を文言だけにする。
fn drop_link_targets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let link = after.find("](").and_then(|close| {
            let label = &after[..close];
            let target_len = link_target_len(&after[close + 2..])?;
            (!label.contains('[')).then_some((label, close + 2 + target_len + 1))
        });
        match link {
            Some((label, consumed)) => {
                out.push_str(&rest[..open]);
                out.push_str(label);
                rest = &after[consumed..];
            }
            None => {
                out.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `http://` / `https://` から空白・非 ASCII・閉じ括弧の手前までを落とす。
fn drop_bare_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = ["https://", "http://"]
        .iter()
        .filter_map(|scheme| rest.find(scheme))
        .min()
    {
        out.push_str(&rest[..start]);
        let url = &rest[start..];
        let end = url
            .find(|c: char| {
                !c.is_ascii() || c.is_ascii_whitespace() || matches!(c, ')' | ']' | '>')
            })
            .unwrap_or(url.len());
        rest = &url[end..];
    }
    out.push_str(rest);
    out
}

/// 「（v<版> で prod 反映）」の括弧の中身。
fn is_prod_stamp(text: &str) -> bool {
    text.strip_prefix('v')
        .and_then(|rest| rest.strip_suffix(crate::CHANGELOG_RELEASED_SUFFIX.trim_end_matches('）')))
        .is_some_and(|version| version.parse::<crate::Version>().is_ok())
}

/// 括弧の中身が日付・未リリース・prod 反映の刻印だけか。
fn is_date_stamp(inner: &str) -> bool {
    match inner.split_once('、') {
        Some((date, "未リリース")) => crate::is_date(date),
        Some((date, stamp)) => crate::is_date(date) && is_prod_stamp(stamp),
        None => crate::is_date(inner) || inner == "未リリース" || is_prod_stamp(inner),
    }
}

/// 中身が日付か刻印だけの全角括弧を落とす。
fn drop_date_stamps(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('（') {
        let inner_start = open + '（'.len_utf8();
        let Some(close) = rest[inner_start..].find('）') else {
            break;
        };
        let inner_end = inner_start + close;
        out.push_str(&rest[..open]);
        if !is_date_stamp(&rest[inner_start..inner_end]) {
            out.push_str(&rest[open..inner_end + '）'.len_utf8()]);
        }
        rest = &rest[inner_end + '）'.len_utf8()..];
    }
    out.push_str(rest);
    out
}

/// 7〜40 桁の小文字の 16 進で、数字と英字の両方を含む語（commit SHA）。数字だけの語は数値として残す。
fn is_commit_sha(word: &str) -> bool {
    (7..=40).contains(&word.len())
        && word
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        && word.bytes().any(|byte| byte.is_ascii_digit())
        && word.bytes().any(|byte| byte.is_ascii_lowercase())
}

fn is_lesson_id(word: &str) -> bool {
    let bytes = word.as_bytes();
    bytes.len() > 1
        && LESSON_ID_PREFIXES.contains(&bytes[0])
        && bytes[1..].iter().all(u8::is_ascii_digit)
}

/// ASCII の英数字の連なりを語として、SHA と lessons の番号を落とす。
fn drop_internal_words(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(|c: char| c.is_ascii_alphanumeric()) {
        out.push_str(&rest[..start]);
        let word = &rest[start..];
        let end = word
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(word.len());
        if !is_commit_sha(&word[..end]) && !is_lesson_id(&word[..end]) {
            out.push_str(&word[..end]);
        }
        rest = &word[end..];
    }
    out.push_str(rest);
    out
}

/// AI へ送る前に、利用者に意味の無い社内の印（日付・prod 反映の刻印・commit SHA・
/// lessons の番号・link の URL）を落とす。別の版の項目を除く `released_for_other_tag` とは
/// 責務が違い、抽出の後に掛ける。
pub fn scrub_for_ai(item: &str) -> String {
    let scrubbed =
        drop_internal_words(&drop_date_stamps(&drop_bare_urls(&drop_link_targets(item))));
    let mut out = String::with_capacity(scrubbed.len());
    for word in scrubbed.split(' ').filter(|word| !word.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// 除去した各項目に 1 始まりの番号を振る。AI の出力の `sources` はこの番号を指す。
pub fn numbered_ai_input(items: &[String]) -> String {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| format!("[{}] {item}\n", index + 1))
        .collect()
}

/// 用途の表の行の名前（`island-llm` の `purpose`）。
const PURPOSE: &str = "release-notes";

/// 推論の実行環境。usage 台帳の置き場は持たず、環境側の設定の `usage_ledger` から決める
/// （他の LLM の呼び出しと同じ台帳に足し、費用の記録を 1 か所にまとめる）。
pub struct AiRuntime<'a> {
    pub home: PathBuf,
    pub op_program: PathBuf,
    pub op_service_account_token: Option<String>,
    pub transport: &'a dyn Transport,
}

/// 宣言から決めた本文の作り方。英語の指示文は未実装なので、宣言がそれを要するときは
/// [`NotesPlan::from_declaration`] が書く前に止める。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotesPlan {
    /// CHANGELOG の抽出をそのまま本文にする。
    Changelog,
    /// 推論を 1 回試し、使えなければ CHANGELOG の抽出に戻す。
    Ai { audience: Audience },
}

impl NotesPlan {
    pub fn from_declaration(notes: &ReleaseNotes) -> Result<Self, String> {
        match notes.mode {
            NotesMode::Changelog(_) => Ok(Self::Changelog),
            NotesMode::Ai { .. } if notes.language != NotesLanguage::Ja => Err(
                "release.notes の language が en の AI 本文はまだ使えません（指示文が日本語だけ）"
                    .to_owned(),
            ),
            NotesMode::Ai { audience, .. } => Ok(Self::Ai {
                audience: match audience {
                    NotesAudience::Private => Audience::Private,
                    NotesAudience::Public => Audience::Public,
                },
            }),
        }
    }
}

/// 宣言の作り方と、抽出の本文（`changelog` の本文か AI の fallback）の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeclaredNotes {
    pub plan: NotesPlan,
    pub source: NotesSource,
}

/// repo の `release.notes` の宣言。無ければ `None`（`release` は軽量 tag を保つ）。
pub fn declared_notes(root: &Path) -> Result<Option<DeclaredNotes>, String> {
    let Some(notes) = crate::preflight::read_declaration(root)?.release_notes else {
        return Ok(None);
    };
    let source = match notes.mode {
        NotesMode::Changelog(source)
        | NotesMode::Ai {
            fallback: source, ..
        } => source,
    };
    Ok(Some(DeclaredNotes {
        plan: NotesPlan::from_declaration(&notes)?,
        source,
    }))
}

/// 版 commit の前に決めた本文の材料。書いた後に失敗しうる処理を減らすため、前 tag・repo と
/// 版の節の有無はここで確かめる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedNotes {
    declared: DeclaredNotes,
    base: ReleaseBase,
    tag: String,
}

/// `HEAD` を版 commit の親として材料を決める。版の節が使えなければ理由 code で止める
/// （AI の宣言でも、fallback の本文が作れない状態で書かない。design §10 の 3）。
pub fn prepare_notes(
    root: &Path,
    declared: DeclaredNotes,
    tag: &str,
) -> Result<PreparedNotes, String> {
    let base = release_base(root, "HEAD")?;
    let prepared = PreparedNotes {
        declared,
        base,
        tag: tag.to_owned(),
    };
    if declared.source == NotesSource::ChangelogSection {
        prepared.section_at(root, "HEAD")?;
    }
    Ok(prepared)
}

impl PreparedNotes {
    pub fn plan(&self) -> NotesPlan {
        self.declared.plan
    }

    /// 終点の commit の CHANGELOG から抽出する。
    pub fn extract(&self, root: &Path, end: &str) -> Result<ReleaseExtract, String> {
        if self.declared.source == NotesSource::ChangelogAdditions {
            return extract_from(root, &self.base, end, &self.tag);
        }
        let ChangelogSection { body, items } = self.section_at(root, end)?;
        let ReleaseBase { previous, repo } = &self.base;
        Ok(ReleaseExtract {
            previous: previous.clone(),
            tag: self.tag.clone(),
            repo: repo.clone(),
            items,
            section: Some(body),
        })
    }

    fn section_at(&self, root: &Path, commit: &str) -> Result<ChangelogSection, String> {
        let path = format!("{commit}:CHANGELOG.md");
        let version = self.tag.strip_prefix('v').unwrap_or(&self.tag);
        let section = match git(root, &["cat-file", "-e", &path]) {
            Ok(_) => extract_changelog_section(&git_raw(root, &["show", &path])?, version),
            Err(_) => Err(SectionError::NoChangelog),
        };
        section.map_err(|error| {
            format!(
                "release.notes の changelog-section を作れません（{}）",
                error
            )
        })
    }
}

/// 本文の出どころ。`release` は本文と一緒にこれを 1 行で出す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodySource {
    /// CHANGELOG の抽出だけの宣言。
    Changelog,
    /// 照合を通った AI の本文。private で辞書が無いまま推論したら `dictionary_missing`。
    Ai {
        dictionary_missing: bool,
    },
    /// AI を使えず fallback の本文にした。理由 code を持つ。
    Fallback(String),
    NoAi,
}

impl std::fmt::Display for BodySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Changelog => write!(f, "CHANGELOG の抽出"),
            Self::Ai {
                dictionary_missing: false,
            } => write!(f, "AI の要約"),
            Self::Ai {
                dictionary_missing: true,
            } => write!(f, "AI の要約（{}）", ai_check::DICTIONARY_MISSING),
            Self::Fallback(code) => write!(f, "fallback（{code}）"),
            Self::NoAi => write!(f, "--no-ai"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseBody {
    pub text: String,
    pub source: BodySource,
}

/// 注釈付き tag の本文を決める。AI の経路の失敗は全部 fallback の本文にし、error を返さない
/// （Release を止めない。design §5 の 5）。推論は `island_llm::run` の 1 回だけで、再試行しない。
pub fn release_body(
    extract: &ReleaseExtract,
    plan: NotesPlan,
    no_ai: bool,
    settings_path: &Path,
    runtime: &AiRuntime<'_>,
) -> ReleaseBody {
    let with = |source| ReleaseBody {
        text: extract.render(),
        source,
    };
    let audience = match plan {
        NotesPlan::Changelog => return with(BodySource::Changelog),
        NotesPlan::Ai { .. } if no_ai => return with(BodySource::NoAi),
        NotesPlan::Ai { audience } => audience,
    };
    match ai_body(extract, audience, settings_path, runtime) {
        Ok(body) => body,
        Err(code) => with(BodySource::Fallback(code)),
    }
}

fn ai_body(
    extract: &ReleaseExtract,
    audience: Audience,
    settings_path: &Path,
    runtime: &AiRuntime<'_>,
) -> Result<ReleaseBody, String> {
    let settings =
        settings::load(settings_path).map_err(|error| error.code(audience).to_owned())?;
    let checks = ai_check::prepare(&settings, audience).map_err(str::to_owned)?;
    if extract.items.is_empty() {
        return Err(Failure::InputRejected.code().to_owned());
    }
    let scrubbed: Vec<String> = extract
        .items
        .iter()
        .map(|item| scrub_for_ai(item))
        .collect();
    let input = numbered_ai_input(&scrubbed);
    // 台帳の置き場が無い古い設定では、費用の記録が残らないので送らない。
    let ledger = settings
        .usage_ledger()
        .ok_or(Failure::UsageUnrecorded.code())?;
    let runtime = island_llm::Runtime {
        home: runtime.home.clone(),
        op_program: runtime.op_program.clone(),
        op_service_account_token: runtime.op_service_account_token.clone(),
        ledger: ledger.to_path_buf(),
        transport: runtime.transport,
    };
    let value = match island_llm::run(PURPOSE, input.as_bytes(), settings.key_ref(), &runtime) {
        Ok(Output::Json(value)) => value,
        Ok(Output::Line(_)) => return Err(Failure::ResponseRejected.code().to_owned()),
        Err(failure) => return Err(failure.code().to_owned()),
    };
    let notes = ai_check::check_output(&value, scrubbed.len(), &checks)?;
    Ok(ReleaseBody {
        text: notes.render(extract),
        source: BodySource::Ai {
            dictionary_missing: checks.dictionary_missing,
        },
    })
}

/// `sha` に注釈付き tag を作る。本文は stdin で渡し、`#` の行も消さない（`--cleanup=verbatim`）。
pub fn annotate_tag(root: &Path, tag: &str, sha: &str, body: &str) -> Result<(), String> {
    use std::io::Write;
    let args = ["tag", "-a", "--cleanup=verbatim", "-F", "-", tag, sha];
    let mut child = develop_tools_core::git(root)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| format!("git を起動できません: {err}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "git の stdin を開けません".to_owned())?
        .write_all(body.as_bytes())
        .map_err(|err| format!("tag の本文を git へ渡せません: {err}"))?;
    let output = child
        .wait_with_output()
        .map_err(|err| format!("git を待てません: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} に失敗しました: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}
