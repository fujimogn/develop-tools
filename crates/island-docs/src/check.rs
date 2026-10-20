//! 検査の組み立て: snapshot を作り、link・容量上限・各検査項目（[`crate::rules`]）を検査し、
//! `--staged` / `--range` なら変わった path に絞る。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::LazyLock;

use develop_core::managed_block;
use regex::Regex;

use crate::constants::{
    ISLAND_BLOCK_PATH, MARKDOWN_EXTENSION, RETIRED_REGISTRY_PATH, SCOPE_COPY_PATH,
    SCRIPT_REFERENCE_CONTEXT_EXCLUSIONS, SCRIPT_REFERENCE_HISTORICAL_EXTRA, ScopeTable,
    allows_host_path_links, byte_limits_for, is_link_source, is_retired_historical, matches_prefix,
};
use crate::git::{Entry, Repo};
use crate::links::{self, BrokenLink, Snapshot};
use crate::rules::RuleFinding;
use crate::{adr, docs, emoji, openspec, records, retired};

/// 見る範囲。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// index の全文書。
    Tracked,
    /// staged で変わった path に関わるものだけ（pre-commit）。
    Staged,
    /// 2 commit 間で変わった path に関わるものだけ（CI）。
    Range { base: String, head: String },
}

/// 検査の入力。git から作るが、検査自体は git に触れない。
#[derive(Debug, Default)]
pub struct Input {
    /// GitHub の `owner/name`。定数表の repo 行を選ぶ。
    pub repo: Option<String>,
    pub snapshot: Snapshot,
    /// 容量上限の対象 path の byte 数。
    pub sizes: BTreeMap<String, usize>,
    /// Markdown 以外で本文を読む file（退役語の registry）。
    pub data: BTreeMap<String, Vec<u8>>,
    /// `--staged` / `--range` のときの変わった path。`None` は全体。
    pub changed: Option<BTreeSet<String>>,
}

/// 見つかった問題 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    Link(BrokenLink),
    ScriptReference {
        path: String,
        line: usize,
        problem: String,
    },
    NotUtf8 {
        path: String,
    },
    OverLimit {
        path: String,
        bytes: usize,
        max_bytes: usize,
    },
    Rule(RuleFinding),
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ScriptReference {
                path,
                line,
                problem,
            } => write!(f, "{path}:{line}: [script] {problem}"),
            Self::Link(link) => write!(
                f,
                "{}:{}: {} -> {}",
                link.source, link.line, link.problem, link.target
            ),
            Self::NotUtf8 { path } => write!(f, "{path}: UTF-8 として読めません"),
            Self::OverLimit {
                path,
                bytes,
                max_bytes,
            } => write!(f, "{path}: 容量上限を超えています {bytes}B > {max_bytes}B"),
            Self::Rule(finding) => finding.fmt(f),
        }
    }
}

/// 検査結果。`checked` は link を検査した文書の数。
#[derive(Debug, Default)]
pub struct Outcome {
    pub findings: Vec<Finding>,
    pub checked: usize,
}

/// `entries`（index か commit tree）にある scope 表の写しを読む。写しが無ければ失敗する。
pub fn scope_table(repo: &Repo, entries: &[Entry]) -> Result<ScopeTable, String> {
    let copy = entries
        .iter()
        .find(|entry| entry.path == SCOPE_COPY_PATH)
        .filter(|entry| entry.is_regular_file())
        .map(|entry| repo.read_blobs(&[&entry.oid]))
        .transpose()?
        .and_then(|mut blobs| blobs.pop());
    ScopeTable::from_copy(repo.root(), copy.as_deref())
}

/// `repo` の index を読んで検査する。
pub fn run(repo: &Repo, mode: Mode) -> Result<Outcome, String> {
    let changed = match &mode {
        Mode::Tracked => None,
        Mode::Staged => Some(repo.staged_changes()?),
        Mode::Range { base, head } => Some(repo.range_changes(base, head)?),
    };
    if changed.as_ref().is_some_and(BTreeSet::is_empty) {
        return Ok(Outcome::default());
    }
    let entries = match &mode {
        Mode::Range { head, .. } => repo.tree_entries(head)?,
        _ => repo.index_entries()?,
    };
    crate::constants::initialize_scopes(scope_table(repo, &entries)?)?;
    let github = repo.github_name();
    let limit_paths: BTreeSet<&str> = byte_limits_for(github.as_deref())
        .map(|limit| limit.path)
        .collect();
    let mut input = Input {
        repo: github.clone(),
        changed,
        ..Input::default()
    };
    let mut to_read = Vec::new();
    for entry in &entries {
        let wanted = entry.path.ends_with(MARKDOWN_EXTENSION)
            || is_package_path(&entry.path)
            || limit_paths.contains(entry.path.as_str())
            || entry.path == RETIRED_REGISTRY_PATH;
        if wanted && entry.is_regular_file() {
            to_read.push(entry);
        } else {
            input.snapshot.add_path(&entry.path);
        }
    }
    let oids: Vec<&str> = to_read.iter().map(|entry| entry.oid.as_str()).collect();
    for (entry, bytes) in to_read.iter().zip(repo.read_blobs(&oids)?) {
        if limit_paths.contains(entry.path.as_str()) {
            input
                .sizes
                .insert(entry.path.clone(), limited_bytes(&entry.path, &bytes));
        }
        if entry.path.ends_with(MARKDOWN_EXTENSION) {
            input.snapshot.add_markdown(&entry.path, bytes);
        } else {
            input.snapshot.add_path(&entry.path);
            if entry.path == RETIRED_REGISTRY_PATH || is_package_path(&entry.path) {
                if is_package_path(&entry.path) {
                    serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|error| {
                        format!("{}: package.json が不正です: {error}", entry.path)
                    })?;
                }
                input.data.insert(entry.path.clone(), bytes);
            }
        }
    }
    Ok(evaluate(&input))
}

/// 容量上限と比べる byte 数。移行中は新旧いずれの管理区画も数えない。
pub fn limited_bytes(path: &str, bytes: &[u8]) -> usize {
    let block = (path == ISLAND_BLOCK_PATH)
        .then(|| managed_block::intact_range(managed_block::AGENTS_MD, bytes))
        .flatten();
    bytes.len() - block.map_or(0, |range| range.len())
}

/// 入力を検査する。
pub fn evaluate(input: &Input) -> Outcome {
    let repo = input.repo.as_deref();
    let touched = |path: &str| input.changed.as_ref().is_none_or(|c| touches(c, path));

    let mut findings = Vec::new();
    let mut sources = Vec::new();
    for (path, text) in input.snapshot.markdown() {
        if !is_link_source(repo, path) {
            continue;
        }
        match text {
            Ok(_) => sources.push(path),
            Err(_) if touched(path) => findings.push(Finding::NotUtf8 {
                path: path.to_owned(),
            }),
            Err(_) => {}
        }
    }
    let checked = sources.len();
    for link in links::check(&input.snapshot, sources, allows_host_path_links(repo)) {
        let relevant = touched(&link.source) || link.resolved.as_deref().is_some_and(touched);
        if relevant {
            findings.push(Finding::Link(link));
        }
    }

    findings.extend(check_script_references(input));

    for limit in byte_limits_for(repo) {
        let Some(bytes) = input.sizes.get(limit.path) else {
            continue;
        };
        if *bytes > limit.max_bytes && touched(limit.path) {
            findings.push(Finding::OverLimit {
                path: limit.path.to_owned(),
                bytes: *bytes,
                max_bytes: limit.max_bytes,
            });
        }
    }

    let rules = [
        docs::check,
        adr::check,
        openspec::check,
        records::check,
        emoji::check,
        retired::check,
    ];
    for finding in rules.iter().flat_map(|rule| rule(input)) {
        let relevant = touched(&finding.path) || finding.related.iter().any(|p| touched(p));
        if relevant {
            findings.push(Finding::Rule(finding));
        }
    }
    Outcome { findings, checked }
}

fn is_package_path(path: &str) -> bool {
    path == "package.json"
        || ["apps/", "packages/"].iter().any(|prefix| {
            path.strip_prefix(prefix)
                .and_then(|rest| rest.split_once('/'))
                .is_some_and(|(name, file)| !name.is_empty() && file == "package.json")
        })
}

static BUN_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bbun[ \t]+run[ \t]+").unwrap());
static SCRIPT_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\b(sh|bash|bun)[ \t]+(?:run[ \t]+)?["']?((?:\./)?scripts/[^\s`"']+)"#).unwrap()
});
static WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?:"[^"]*"|'[^']*'|[^\s`]+)"#).unwrap());
static CD_WORKSPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bcd\s+((?:apps|packages)/[^\s;&]+)\s*&&\s*$").unwrap());

fn placeholder(word: &str) -> bool {
    word.contains(['<', '>', '$', '*']) || word.contains("...")
}

fn command_word(word: &str) -> &str {
    word.trim_start_matches(['`', '\'', '"', '（', '('])
        .trim_end_matches(['`', '\'', '"', '）', ')', '.', ',', ';', '。'])
}

fn external_repo_context(prefix: &str) -> bool {
    prefix.contains("子 repo") || prefix.contains("他 repo") || prefix.contains("配布する")
}

fn source_workspace(source: &str, packages: &BTreeMap<&str, BTreeSet<String>>) -> Option<String> {
    let (kind, rest) = source.split_once('/')?;
    if kind != "apps" && kind != "packages" {
        return None;
    }
    let (name, _) = rest.split_once('/')?;
    let workspace = format!("{kind}/{name}");
    packages
        .contains_key(format!("{workspace}/package.json").as_str())
        .then_some(workspace)
}

fn check_script_references(input: &Input) -> Vec<Finding> {
    if !input.data.contains_key("package.json") {
        return Vec::new();
    }
    let mut packages = BTreeMap::new();
    let mut names = BTreeMap::new();
    for (path, bytes) in input.data.iter().filter(|(path, _)| is_package_path(path)) {
        let Ok(pkg) = serde_json::from_slice::<serde_json::Value>(bytes) else {
            continue;
        };
        let scripts: BTreeSet<String> = pkg
            .get("scripts")
            .and_then(serde_json::Value::as_object)
            .map(|scripts| scripts.keys().cloned().collect())
            .unwrap_or_default();
        if path != "package.json" {
            let workspace = path.strip_suffix("/package.json").unwrap();
            if let Some(name) = pkg.get("name").and_then(serde_json::Value::as_str) {
                names.insert(name.to_owned(), workspace.to_owned());
            }
        }
        packages.insert(path.as_str(), scripts);
    }
    if !packages.contains_key("package.json") {
        return Vec::new();
    }

    let changed = input.changed.as_ref();
    let tracked: BTreeSet<&str> = input.snapshot.files().collect();
    let mut findings = Vec::new();
    for (source, text) in input.snapshot.markdown() {
        let repo = input.repo.as_deref();
        if is_retired_historical(repo, source)
            || matches_prefix(SCRIPT_REFERENCE_HISTORICAL_EXTRA, repo, source)
            || matches_prefix(SCRIPT_REFERENCE_CONTEXT_EXCLUSIONS, repo, source)
            || !is_link_source(input.repo.as_deref(), source)
        {
            continue;
        }
        let Ok(text) = text else { continue };
        let source_workspace = source_workspace(source, &packages);
        let mut missing_paths = BTreeSet::new();
        for (index, line) in text.lines().enumerate() {
            let line_number = index + 1;
            for match_ in SCRIPT_PATH.captures_iter(line) {
                if external_repo_context(&line[..match_.get(0).unwrap().start()]) {
                    continue;
                }
                let raw = match_.get(2).unwrap().as_str();
                if placeholder(raw) {
                    continue;
                }
                let path = command_word(raw)
                    .strip_prefix("./")
                    .unwrap_or(command_word(raw));
                if match_.get(1).unwrap().as_str() == "bun" && !path.ends_with(".ts") {
                    continue;
                }
                let workspace_path = source_workspace
                    .as_deref()
                    .map(|workspace| format!("{workspace}/{path}"));
                let target = workspace_path
                    .as_deref()
                    .filter(|target| tracked.contains(*target))
                    .unwrap_or(path);
                if !tracked.contains(target)
                    && (relevant(changed, source, path)
                        || workspace_path.as_deref().is_some_and(|workspace_path| {
                            relevant(changed, source, workspace_path)
                        }))
                    && missing_paths.insert(path.to_owned())
                {
                    findings.push(Finding::ScriptReference {
                        path: source.to_owned(),
                        line: line_number,
                        problem: format!("script path が tracked にありません -> {path}"),
                    });
                }
            }
            for match_ in BUN_RUN.find_iter(line) {
                if external_repo_context(&line[..match_.start()]) {
                    continue;
                }
                let rest = &line[match_.end()..];
                let words: Vec<&str> = WORD.find_iter(rest).map(|word| word.as_str()).collect();
                let mut workspace: Option<String> = None;
                let mut unknown_workspace = None;
                let mut script = None;
                let mut skip = false;
                let mut source_fallback = false;
                let mut index = 0;
                while index < words.len() {
                    let raw = words[index];
                    let word = command_word(raw);
                    if word.starts_with("--filter") || word.starts_with("--cwd") {
                        let value = if let Some((_, value)) = raw.split_once('=') {
                            value
                        } else {
                            index += 1;
                            words.get(index).copied().unwrap_or("")
                        };
                        if placeholder(value) {
                            skip = true;
                            break;
                        }
                        let value = command_word(value);
                        if word.starts_with("--filter") {
                            workspace = names.get(value).cloned();
                            if workspace.is_none() {
                                unknown_workspace = Some(value.to_owned());
                            }
                        } else {
                            workspace = Some(value.strip_prefix("./").unwrap_or(value).to_owned());
                        }
                    } else if !word.starts_with("--") {
                        script = Some(word);
                        break;
                    }
                    index += 1;
                }
                let Some(script) = script else { continue };
                if skip
                    || placeholder(script)
                    || script.contains(['/', '|', '\\', '。'])
                    || script.starts_with('-')
                {
                    continue;
                }
                if let Some(name) = unknown_workspace {
                    if relevant(changed, source, "package.json")
                        || changed
                            .is_some_and(|paths| paths.iter().any(|path| is_package_path(path)))
                    {
                        findings.push(Finding::ScriptReference {
                            path: source.to_owned(),
                            line: line_number,
                            problem: format!("workspace script がありません -> {name}:{script}"),
                        });
                    }
                    continue;
                }
                if workspace.is_none() {
                    workspace = CD_WORKSPACE
                        .captures(&line[..match_.start()])
                        .map(|capture| capture[1].to_owned());
                }
                if workspace.is_none()
                    && !packages
                        .get("package.json")
                        .is_some_and(|scripts| scripts.contains(script))
                {
                    workspace = source_workspace.clone();
                    source_fallback = workspace.is_some();
                }
                let package_path = workspace.as_ref().map_or_else(
                    || "package.json".to_owned(),
                    |path| format!("{path}/package.json"),
                );
                if !packages
                    .get(package_path.as_str())
                    .is_some_and(|scripts| scripts.contains(script))
                    && (relevant(changed, source, &package_path)
                        || (source_fallback && relevant(changed, source, "package.json")))
                {
                    let name = workspace
                        .map_or_else(|| script.to_owned(), |path| format!("{path}:{script}"));
                    findings.push(Finding::ScriptReference {
                        path: source.to_owned(),
                        line: line_number,
                        problem: format!("bun run script がありません -> {name}"),
                    });
                }
            }
        }
    }
    findings
}

fn relevant(changed: Option<&BTreeSet<String>>, source: &str, target: &str) -> bool {
    changed.is_none_or(|changed| touches(changed, source) || touches(changed, target))
}

/// `path` 自身か、`path` を dir とする path が変わったか。
fn touches(changed: &BTreeSet<String>, path: &str) -> bool {
    if changed.contains(path) {
        return true;
    }
    let prefix = format!("{path}/");
    changed
        .range(prefix.clone()..)
        .next()
        .is_some_and(|next| next.starts_with(&prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(files: &[(&str, &str)], changed: Option<&[&str]>) -> Input {
        let mut input = Input::default();
        for (path, text) in files {
            input.snapshot.add_markdown(path, text.as_bytes().to_vec());
        }
        input.changed = changed.map(|c| c.iter().map(|p| (*p).to_owned()).collect());
        input
    }

    /// link と容量上限だけ（検査項目の結果は各 module のテストが見る）。
    fn sources(outcome: &Outcome) -> Vec<String> {
        outcome
            .findings
            .iter()
            .filter(|finding| !matches!(finding, Finding::Rule(_)))
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn staged_reports_links_from_changed_sources_and_into_changed_targets() {
        let files = [
            ("a.md", "[b](b.md#gone)\n"),
            ("b.md", "# kept\n"),
            ("c.md", "[x](missing.md)\n"),
        ];
        // b.md の見出しを書き換えた commit: a.md は触っていないが、link が切れるので報告する。
        let outcome = evaluate(&input(&files, Some(&["b.md"])));
        assert_eq!(
            sources(&outcome),
            ["a.md:1: リンク先に見出しがありません -> b.md#gone"]
        );
        // 全体なら c.md の既存の切れも出る。
        assert_eq!(sources(&evaluate(&input(&files, None))).len(), 2);
    }

    #[test]
    fn staged_reports_links_into_a_removed_directory() {
        let files = [("a.md", "[d](docs/)\n")];
        let outcome = evaluate(&input(&files, Some(&["docs/x.md"])));
        assert_eq!(
            sources(&outcome),
            ["a.md:1: リンク先が tracked にありません -> docs/"]
        );
    }

    #[test]
    fn limits_come_from_the_island_table_for_the_repo() {
        let mut limited = input(&[], None);
        limited.repo = Some("example/home".to_owned());
        limited.sizes.insert("home/AGENTS.md".to_owned(), 12_001);
        limited.sizes.insert("TODO.md".to_owned(), 36_000);
        assert_eq!(
            sources(&evaluate(&limited)),
            ["home/AGENTS.md: 容量上限を超えています 12001B > 12000B"]
        );
        // 表に example/home の行しか無い path は、他の repo では上限の対象外。
        limited.repo = Some("owner/other".to_owned());
        assert!(sources(&evaluate(&limited)).is_empty());
    }

    /// `--staged` は検査項目も変わった path（と関係する path）に絞る。索引から外れた文書は、文書か
    /// 索引のどちらかが変わったときに報告する。
    #[test]
    fn staged_rule_findings_follow_the_changed_and_related_paths() {
        let files = [
            ("docs/README.md", "# d\n"),
            ("docs/a.md", "# a\n"),
            ("TODO.md", "# TODO\n- [x] **済**\n"),
        ];
        let rules = |changed: &[&str]| -> Vec<String> {
            evaluate(&input(&files, Some(changed)))
                .findings
                .iter()
                .filter_map(|finding| match finding {
                    Finding::Rule(rule) => Some(format!("{}:{}", rule.rule.label(), rule.path)),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(rules(&["docs/README.md"]), ["index:docs/a.md"]);
        assert_eq!(rules(&["TODO.md"]), ["todo:TODO.md"]);
        assert!(rules(&["src/main.rs"]).is_empty());
    }

    /// 写した参照文書の link は見ないが、写しを指す link は見る。
    #[test]
    fn copied_references_are_not_checked_in_their_repo() {
        let files = [
            (
                "docs/references/upstream/model.md",
                "[x](../../domain/x.md)\n",
            ),
            ("docs/README.md", "[m](references/upstream/gone.md)\n"),
        ];
        let mut site = input(&files, None);
        site.repo = Some("example/site".to_owned());
        assert_eq!(
            sources(&evaluate(&site)),
            ["docs/README.md:1: リンク先が tracked にありません -> references/upstream/gone.md"]
        );
        let mut other = input(&files, None);
        other.repo = Some("owner/other".to_owned());
        assert_eq!(sources(&evaluate(&other)).len(), 2);
    }
}
