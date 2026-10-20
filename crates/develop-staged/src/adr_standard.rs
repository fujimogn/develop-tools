//! ADR の標準構造・採番・索引の検査（`check-project-integrity` の ADR lane）。
//!
//! **移設元の TS の ADR 標準は 2 用途を持つ 1 ファイル**で、template の plan / apply は
//! `sync-adr-template` の担当として `develop-sync` へ移設済み。ここは検査側だけを持つ。
//! TS で 1 ファイルだったものが消費者で割れる形は、この移設で初めて出た。構造の整理は 6.x。
//!
//! legacy archive の検証（SHA-256 照合）は `develop-sync` 側に実体があるため、
//! **関数として受け取る**形にしてある。どこから持ってくるかは配線側の判断で、
//! この module 自体はそれに依存しない。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

pub const ADR_TEMPLATE_RELATIVE_PATH: &str = "docs/adr/adr-template.md";

/// legacy ADR 移行時に 169 件へ一括投入された Confirmation 文言。
///
/// 新規 ADR がこれをコピーしていたら、その ADR 固有の検証手段が書かれていない
/// (= Confirmation が形骸化した) サインなので違反にする。ADR-0013 の legacy 移行由来。
/// **部分一致で判定する**のは、この日本語フレーズが移行専用で、正当な新規 ADR が
/// Confirmation にそのまま書く現実的シナリオが無いため。言い換えた boilerplate は追わない。
const LEGACY_CONFIRMATION_MARKER: &str = "移行前原文のSHA-256を照合";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub code: &'static str,
    pub message: String,
}

fn issue(code: &'static str, message: impl Into<String>) -> Issue {
    Issue {
        code,
        message: message.into(),
    }
}

pub struct Options<'a> {
    pub project_root: &'a Path,
    pub canonical_template: &'a str,
    pub migrated_through: Option<u32>,
    pub reserved_numbers: &'a [u32],
    pub sanitized_numbers: &'a [u32],
}

/// legacy archive の検証。実体は `develop-sync` にあるので**注入して使う**。
pub struct ArchiveValidators<'a> {
    pub archive: &'a dyn Fn(&str) -> bool,
    pub sanitized: &'a dyn Fn(&str) -> bool,
}

/// `### Confirmation` 節の本文だけを取り出す。
///
/// 次の見出しの手前まで。HTML コメントは除去する。template はコメントが先・本文が後の順なので、
/// **コメントで打ち切らずに除去してから**本文を返す。
pub fn extract_confirmation_section(content: &str) -> Option<String> {
    let normalized = content.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let start = lines
        .iter()
        .position(|line| line.trim() == "### Confirmation")?;

    let mut body = Vec::new();
    for line in &lines[start + 1..] {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") || trimmed.starts_with("### ") {
            break;
        }
        body.push(*line);
    }
    Some(strip_html_comments(&body.join("\n")).trim().to_string())
}

fn strip_html_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 4..];
        match tail.find("-->") {
            Some(end) => rest = &tail[end + 3..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

pub(crate) use island_docs::adr::adr_file_number;

/// staged変更前後のADR番号→path集合を比較し、今回増えた採番違反を返す。
/// 既存欠番や変更番号と無関係な重複は拒否しない。
pub(crate) fn inspect_number_set_transition(
    head: &BTreeMap<u32, BTreeSet<String>>,
    index: &BTreeMap<u32, BTreeSet<String>>,
    changed_paths: &[String],
    reserved_numbers: &[u32],
) -> Vec<Issue> {
    let mut changed_numbers = BTreeSet::new();
    for path in changed_paths {
        if let Some(number) = adr_path_number(path) {
            changed_numbers.insert(number);
        }
    }
    let mut issues = Vec::new();
    for number in &changed_numbers {
        if index.get(number).is_some_and(|paths| paths.len() > 1) {
            issues.push(issue(
                "duplicate-adr-number",
                format!(
                    "staged変更と関係するADR-{number:04} がindex内で重複しています: {}",
                    index[number].iter().cloned().collect::<Vec<_>>().join(", ")
                ),
            ));
        }
    }
    for (number, paths) in head {
        if !index.contains_key(number) {
            issues.push(issue(
                "removed-adr-number",
                format!(
                    "HEADに存在する番号がindexから消えています: ADR-{number:04} ({})",
                    paths.iter().cloned().collect::<Vec<_>>().join(", ")
                ),
            ));
        }
    }

    let head_missing = missing_adr_numbers(head, reserved_numbers);
    let index_missing = missing_adr_numbers(index, reserved_numbers);
    for number in index_missing.difference(&head_missing) {
        issues.push(issue(
            "nonsequential-adr-number",
            format!("HEADから増えた番号の欠番です: ADR-{number:04}"),
        ));
    }
    issues
}

fn adr_path_number(path: &str) -> Option<u32> {
    let path = Path::new(path);
    if path.parent() != Some(Path::new("docs/adr")) {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    if name == "README.md" || name == "adr-template.md" {
        return None;
    }
    adr_file_number(name)?.parse().ok()
}

fn missing_adr_numbers(
    numbers: &BTreeMap<u32, BTreeSet<String>>,
    reserved_numbers: &[u32],
) -> BTreeSet<u32> {
    let Some(highest) = numbers.keys().max().copied() else {
        return BTreeSet::new();
    };
    (1..=highest)
        .filter(|number| !numbers.contains_key(number) && !reserved_numbers.contains(number))
        .collect()
}

/// 新規 ADR が標準構造を満たすか。判定は島の docs 検査と共有する（`island_docs::adr`）。
fn valid_adr_document(content: &str, number: &str) -> bool {
    island_docs::adr::document_problem(content, number).is_none()
}

pub fn inspect(options: &Options, validators: &ArchiveValidators) -> Vec<Issue> {
    let adr_dir = options.project_root.join("docs").join("adr");
    let template_path = options.project_root.join(ADR_TEMPLATE_RELATIVE_PATH);
    let mut issues = Vec::new();

    match fs::read_to_string(&template_path) {
        Err(_) => issues.push(issue(
            "missing-adr-template",
            format!("{ADR_TEMPLATE_RELATIVE_PATH} がありません"),
        )),
        Ok(template) if template != options.canonical_template => issues.push(issue(
            "adr-template-drift",
            format!("{ADR_TEMPLATE_RELATIVE_PATH} が島の正本と不一致です"),
        )),
        Ok(_) => {}
    }

    if !adr_dir.exists() {
        return issues;
    }

    let readme = fs::read_to_string(adr_dir.join("README.md")).ok();
    if readme.is_none() {
        issues.push(issue(
            "missing-adr-readme",
            "docs/adr/README.md がありません",
        ));
    }
    let readme = readme.unwrap_or_default();

    // 番号 → ファイル名。重複と欠番の判定に使う。BTreeMap で報告順を安定させる。
    let mut numbers: BTreeMap<u32, Vec<String>> = BTreeMap::new();

    let entries = match fs::read_dir(&adr_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return issues,
        Err(error) => {
            issues.push(issue(
                "adr-directory-read-error",
                format!("docs/adr の列挙に失敗しました: {error}"),
            ));
            return issues;
        }
    };
    let mut files = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".md") {
                    files.push(name);
                }
            }
            Err(error) => issues.push(issue(
                "adr-directory-read-error",
                format!("docs/adr の entry 読取に失敗しました: {error}"),
            )),
        }
    }
    files.sort();

    for file in files {
        if file == "README.md" || file == "adr-template.md" {
            continue;
        }
        let Some(number_text) = adr_file_number(&file) else {
            issues.push(issue(
                "invalid-adr-filename",
                format!("ADR filename が NNNN-kebab-case.md ではありません: {file}"),
            ));
            continue;
        };
        let number: u32 = number_text.parse().unwrap_or(0);
        numbers.entry(number).or_default().push(file.clone());

        if options.reserved_numbers.contains(&number) {
            issues.push(issue(
                "reused-reserved-adr-number",
                format!("legacy採番時点の欠番は再利用できません: ADR-{number_text}"),
            ));
        }

        let content = match fs::read_to_string(adr_dir.join(&file)) {
            Ok(content) => content,
            Err(error) => {
                issues.push(issue(
                    "adr-body-read-error",
                    format!("docs/adr/{file} の本文読取に失敗しました: {error}"),
                ));
                continue;
            }
        };
        if !valid_adr_document(&content, number_text) {
            issues.push(issue(
                "invalid-adr-structure",
                format!("新規 ADR が標準構造を満たしません: {file}"),
            ));
        }

        let is_migrated = options
            .migrated_through
            .is_some_and(|boundary| number <= boundary);
        let valid_archive = !is_migrated
            || if options.sanitized_numbers.contains(&number) {
                (validators.sanitized)(&content)
            } else {
                (validators.archive)(&content)
            };
        if !valid_archive {
            issues.push(issue(
                "invalid-legacy-archive",
                format!("移行済みADRのOriginal RecordまたはSHA-256が不正です: {file}"),
            ));
        }

        // migratedThrough より上 = 新規 ADR。Confirmation が移行 boilerplate のままなら
        // 固有の検証手段が未記入。ADR 固有のテスト / CI / 監査 / 実機手順を書かせる。
        let is_new = options
            .migrated_through
            .is_none_or(|boundary| number > boundary);
        if is_new {
            if let Some(confirmation) = extract_confirmation_section(&content) {
                if confirmation.contains(LEGACY_CONFIRMATION_MARKER) {
                    issues.push(issue(
                        "boilerplate-confirmation",
                        format!("新規 ADR の Confirmation が移行 boilerplate のままです。固有の検証手段 (テスト名 / CI ジョブ / 監査コマンド / 実機手順) を書いてください: {file}"),
                    ));
                }
            }
        }

        if !readme.contains(&file) {
            issues.push(issue(
                "unindexed-adr",
                format!("新規 ADR を README 索引へ1回掲載してください: {file}"),
            ));
        }
    }

    for (number, files) in &numbers {
        if files.len() > 1 {
            issues.push(issue(
                "duplicate-adr-number",
                format!("ADR-{number:04} が重複しています: {}", files.join(", ")),
            ));
        }
    }

    // 欠番の検出。予約番号は欠けていてよい（legacy 採番時点の欠番）。
    if let Some(highest) = numbers.keys().max().copied() {
        for number in 1..=highest {
            if !numbers.contains_key(&number) && !options.reserved_numbers.contains(&number) {
                issues.push(issue(
                    "nonsequential-adr-number",
                    format!("新規ADRは連続採番してください: ADR-{number:04} がありません"),
                ));
            }
        }
    }

    issues
}

/// staged された ADR ファイル（`docs/adr/` 直下の名前と内容）。
pub struct StagedFile {
    pub name: String,
    pub content: String,
    pub deleted: bool,
}

/// worktree 全件ではなく、staged された ADR だけを `inspect` と同じ判定に通す。
///
/// 連番欠番は全 tree の性質なので見ない（未 staged の既存欠番で無関係な commit を止めない）。
/// template drift も未 staged なら `check-project-integrity` に残す。
pub fn inspect_staged(
    options: &Options,
    validators: &ArchiveValidators,
    staged: &[StagedFile],
    readme: Option<&str>,
) -> Vec<Issue> {
    inspect_staged_with_index(options, validators, staged, readme, None)
}

pub fn inspect_staged_with_index(
    options: &Options,
    validators: &ArchiveValidators,
    staged: &[StagedFile],
    readme: Option<&str>,
    index_file: Option<&Path>,
) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut numbering_issues = Vec::new();
    if staged.iter().any(|file| {
        file.name.ends_with(".md") && file.name != "README.md" && file.name != "adr-template.md"
    }) && options.project_root.join(".git").exists()
    {
        match crate::staged_adr::inspect_numbering_transition(
            options.project_root,
            options.reserved_numbers,
            staged,
            index_file,
        ) {
            Ok(found) => numbering_issues = found,
            Err(error) => numbering_issues.push(issue("staged-adr-numbering-git-error", error)),
        }
    }

    let adr_files: Vec<&StagedFile> = staged
        .iter()
        .filter(|file| !file.deleted && file.name != "README.md" && file.name != "adr-template.md")
        .collect();
    if adr_files.is_empty() {
        return numbering_issues;
    }

    // 呼び出し側が staged blob または HEAD を渡す。worktree の dirty README は見ない。
    if readme.is_none() {
        issues.push(issue(
            "missing-adr-readme",
            "docs/adr/README.md がありません",
        ));
    }
    let readme = readme.unwrap_or_default();

    let mut numbers: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for file in adr_files {
        let Some(number_text) = adr_file_number(&file.name) else {
            issues.push(issue(
                "invalid-adr-filename",
                format!(
                    "ADR filename が NNNN-kebab-case.md ではありません: {}",
                    file.name
                ),
            ));
            continue;
        };
        let number: u32 = number_text.parse().unwrap_or(0);
        numbers.entry(number).or_default().push(file.name.clone());

        if options.reserved_numbers.contains(&number) {
            issues.push(issue(
                "reused-reserved-adr-number",
                format!("legacy採番時点の欠番は再利用できません: ADR-{number_text}"),
            ));
        }

        if !valid_adr_document(&file.content, number_text) {
            issues.push(issue(
                "invalid-adr-structure",
                format!("新規 ADR が標準構造を満たしません: {}", file.name),
            ));
        }

        let is_migrated = options
            .migrated_through
            .is_some_and(|boundary| number <= boundary);
        let valid_archive = !is_migrated
            || if options.sanitized_numbers.contains(&number) {
                (validators.sanitized)(&file.content)
            } else {
                (validators.archive)(&file.content)
            };
        if !valid_archive {
            issues.push(issue(
                "invalid-legacy-archive",
                format!(
                    "移行済みADRのOriginal RecordまたはSHA-256が不正です: {}",
                    file.name
                ),
            ));
        }

        let is_new = options
            .migrated_through
            .is_none_or(|boundary| number > boundary);
        if is_new {
            if let Some(confirmation) = extract_confirmation_section(&file.content) {
                if confirmation.contains(LEGACY_CONFIRMATION_MARKER) {
                    issues.push(issue(
                        "boilerplate-confirmation",
                        format!("新規 ADR の Confirmation が移行 boilerplate のままです。固有の検証手段 (テスト名 / CI ジョブ / 監査コマンド / 実機手順) を書いてください: {}", file.name),
                    ));
                }
            }
        }

        if !readme.contains(&file.name) {
            issues.push(issue(
                "unindexed-adr",
                format!(
                    "新規 ADR を README 索引へ1回掲載してください: {}",
                    file.name
                ),
            ));
        }
    }

    issues.extend(numbering_issues);
    for (number, files) in &numbers {
        if files.len() > 1 {
            issues.push(issue(
                "duplicate-adr-number",
                format!("ADR-{number:04} が重複しています: {}", files.join(", ")),
            ));
        }
    }

    issues
}

#[cfg(test)]
mod tests;
