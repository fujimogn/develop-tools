//! staged された `docs/adr/` を `adr_standard::inspect_staged` へ通す。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use develop_core::develop_tools_manifest::{self, AdrContract};
use develop_core::git;

use crate::adr_standard::{Issue, StagedFile, adr_file_number, inspect_number_set_transition};

/// `git diff --cached` の docs/adr 配下。削除は本文を読まず番号差分へ渡す。
pub fn list_staged(repo: &Path) -> Result<Vec<StagedFile>, String> {
    list_staged_with_index(repo, None)
}

pub fn list_staged_with_index(
    repo: &Path,
    index_file: Option<&Path>,
) -> Result<Vec<StagedFile>, String> {
    if has_unmerged_adr_entry(repo, index_file)? {
        return Err("unmerged-index: index に未解決の競合があります".to_string());
    }
    let names = git_stdout_with_index(
        repo,
        index_file,
        &[
            "diff",
            "--cached",
            "--no-renames",
            "--diff-filter=ACDMRT",
            "--name-only",
            "-z",
            "--",
            "docs/adr",
        ],
    )?;
    let deleted = git_stdout_with_index(
        repo,
        index_file,
        &[
            "diff",
            "--cached",
            "--no-renames",
            "--diff-filter=D",
            "--name-only",
            "-z",
            "--",
            "docs/adr",
        ],
    )?
    .split('\0')
    .filter(|name| !name.is_empty())
    .map(str::to_string)
    .collect::<BTreeSet<_>>();
    let mut files = Vec::new();
    for raw in names.split('\0') {
        if raw.is_empty() {
            continue;
        }
        let path = Path::new(raw);
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if path.parent() != Some(Path::new("docs/adr")) {
            continue;
        }
        if !name.ends_with(".md") {
            continue;
        }
        let is_deleted = deleted.contains(raw);
        let content = if is_deleted {
            String::new()
        } else {
            git_blob_with_index(repo, index_file, raw)?
        };
        files.push(StagedFile {
            name: name.to_string(),
            content,
            deleted: is_deleted,
        });
    }
    Ok(files)
}

fn has_unmerged_adr_entry(repo: &Path, index_file: Option<&Path>) -> Result<bool, String> {
    let entries = git_stdout_with_index(
        repo,
        index_file,
        &["ls-files", "--unmerged", "-z", "--", "docs/adr"],
    )?;
    Ok(!entries.is_empty())
}

pub fn staged_readme(repo: &Path, staged: &[StagedFile]) -> Option<String> {
    staged_readme_with_index(repo, staged, None)
}

pub fn staged_readme_with_index(
    repo: &Path,
    staged: &[StagedFile],
    index_file: Option<&Path>,
) -> Option<String> {
    if let Some(file) = staged
        .iter()
        .find(|file| file.name == "README.md" && !file.deleted)
    {
        return Some(file.content.clone());
    }
    if staged
        .iter()
        .any(|file| file.name == "README.md" && file.deleted)
        || staged_path_deleted(repo, "docs/adr/README.md", index_file)
    {
        return None;
    }
    // 未 staged の worktree は次の commit に入らない。HEAD の索引を見る。
    git_stdout(repo, &["show", "HEAD:docs/adr/README.md"]).ok()
}

/// 明示 index の path を解決・検証し、canonicalize された絶対 path を返す。
///
/// 相対 path は `cwd` を基準に解決され、通常ファイルであること、
/// および現在の worktree の Git directory 配下であることが検証される。
pub fn resolve_index_file(cwd: &Path, repo: &Path, raw_path: &Path) -> Result<PathBuf, String> {
    let unresolved_path = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        cwd.join(raw_path)
    };
    let index_file = fs::canonicalize(&unresolved_path).map_err(|error| {
        format!(
            "明示 index を読み取れません: {}: {error}",
            unresolved_path.display()
        )
    })?;
    if !index_file.is_file() {
        return Err(format!(
            "明示 index は通常ファイルである必要があります: {}",
            index_file.display()
        ));
    }

    let output = git(repo)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .map_err(|error| format!("git rev-parse --absolute-git-dir: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git rev-parse --absolute-git-dir failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let git_dir = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let git_dir = fs::canonicalize(&git_dir)
        .map_err(|error| format!("current worktree git dir {}: {error}", git_dir.display()))?;
    if !index_file.starts_with(&git_dir) {
        return Err(format!(
            "明示 index は現在の worktree の Git directory 配下である必要があります: {}",
            index_file.display()
        ));
    }
    Ok(index_file)
}

/// Compare complete `HEAD` and index ADR-number maps for a staged ADR change.
pub(crate) fn inspect_numbering_transition(
    repo: &Path,
    reserved_numbers: &[u32],
    staged: &[StagedFile],
    index_file: Option<&Path>,
) -> Result<Vec<Issue>, String> {
    let changed_paths = staged
        .iter()
        .filter(|file| is_adr_markdown_path(&format!("docs/adr/{}", file.name)))
        .map(|file| format!("docs/adr/{}", file.name))
        .collect::<Vec<_>>();
    if changed_paths.is_empty() {
        return Ok(Vec::new());
    }

    let head_paths = head_adr_paths(repo)?;
    let index_paths = index_adr_paths(repo, index_file)?;
    let head = number_paths(&head_paths);
    let index = number_paths(&index_paths);
    Ok(inspect_number_set_transition(
        &head,
        &index,
        &changed_paths,
        reserved_numbers,
    ))
}

fn is_adr_markdown_path(path: &str) -> bool {
    let path = Path::new(path);
    path.parent() == Some(Path::new("docs/adr"))
        && path.extension().is_some_and(|extension| extension == "md")
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name != "README.md" && name != "adr-template.md")
}

fn head_adr_paths(repo: &Path) -> Result<Vec<String>, String> {
    let head_tree = git_output(repo, &["rev-parse", "--verify", "--quiet", "HEAD^{tree}"])?;
    if !head_tree.status.success() {
        if is_unborn_branch(repo)? {
            return Ok(Vec::new());
        }
        return Err(format!(
            "HEAD tree could not be read: {}",
            String::from_utf8_lossy(&head_tree.stderr).trim()
        ));
    }
    let paths = git_stdout(
        repo,
        &[
            "ls-tree",
            "-r",
            "--name-only",
            "-z",
            "HEAD",
            "--",
            "docs/adr",
        ],
    )?;
    Ok(paths
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect())
}

fn is_unborn_branch(repo: &Path) -> Result<bool, String> {
    let symbolic = git_output(repo, &["symbolic-ref", "-q", "HEAD"])?;
    if !symbolic.status.success() {
        return Err(format!(
            "HEAD is not a readable symbolic unborn branch: {}",
            String::from_utf8_lossy(&symbolic.stderr).trim()
        ));
    }
    let reference = String::from_utf8(symbolic.stdout)
        .map_err(|error| format!("symbolic-ref output is not UTF-8: {error}"))?;
    let reference = reference.trim();
    let exists = git_output(repo, &["show-ref", "--verify", "--quiet", reference])?;
    match exists.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(format!(
            "cannot determine whether unborn branch ref exists: {}",
            String::from_utf8_lossy(&exists.stderr).trim()
        )),
    }
}

fn index_adr_paths(repo: &Path, index_file: Option<&Path>) -> Result<Vec<String>, String> {
    let entries = git_stdout_with_index(
        repo,
        index_file,
        &["ls-files", "--stage", "-z", "--", "docs/adr"],
    )?;
    let mut paths = Vec::new();
    for entry in entries.split('\0').filter(|entry| !entry.is_empty()) {
        let (metadata, path) = entry
            .split_once('\t')
            .ok_or_else(|| "git ls-files returned a malformed index entry".to_string())?;
        let stage = metadata
            .split_ascii_whitespace()
            .nth(2)
            .ok_or_else(|| "git ls-files returned a malformed index stage".to_string())?;
        if stage != "0" {
            return Err(format!("index contains unmerged entry at {path}"));
        }
        paths.push(path.to_string());
    }
    Ok(paths)
}

fn number_paths(paths: &[String]) -> BTreeMap<u32, BTreeSet<String>> {
    let mut numbers = BTreeMap::new();
    for path in paths {
        let path_obj = Path::new(path);
        if path_obj.parent() != Some(Path::new("docs/adr")) {
            continue;
        }
        let Some(name) = path_obj.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name == "README.md" || name == "adr-template.md" {
            continue;
        }
        let Some(number) = adr_file_number(name).and_then(|number| number.parse::<u32>().ok())
        else {
            continue;
        };
        numbers
            .entry(number)
            .or_insert_with(BTreeSet::new)
            .insert(path.clone());
    }
    numbers
}

fn staged_path_deleted(repo: &Path, path: &str, index_file: Option<&Path>) -> bool {
    git_stdout_with_index(
        repo,
        index_file,
        &[
            "diff",
            "--cached",
            "--diff-filter=D",
            "--name-only",
            "-z",
            "--",
            path,
        ],
    )
    .map(|names| names.split('\0').any(|name| name == path))
    .unwrap_or(false)
}

/// repo の ADR 番号の契約。staged の検査なので、作業ツリーではなく index の
/// `.develop-tools.json` を読む。宣言が無い・契約の欄が無い・壊れているときは失敗する。
pub fn adr_contract(repo: &Path, index_file: Option<&Path>) -> Result<AdrContract, String> {
    let path = develop_tools_manifest::PATH;
    let listed = git_stdout_with_index(
        repo,
        index_file,
        &["ls-files", "-z", "--cached", "--", path],
    )?;
    if listed.is_empty() {
        return Err(format!(
            "{path} が index に無い（ADR 番号の契約の欄は配布の道具が書く。手で足さない）"
        ));
    }
    let output = git_output_with_index(repo, index_file, &["show", &format!(":{path}")])?;
    if !output.status.success() {
        return Err(format!(
            "git show :{path} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("{path}（index）: {error}"))?;
    develop_tools_manifest::adr_contract(&value)
        .map_err(|error| format!("{path}（index）: {error}"))?
        .ok_or_else(|| format!(
            "{path}（index）に ADR 番号の契約の欄が無い（この欄は配布の道具が書く。手で足さない）"
        ))
}

fn git_blob_with_index(
    repo: &Path,
    index_file: Option<&Path>,
    path: &str,
) -> Result<String, String> {
    let output = git_output_with_index(repo, index_file, &["show", &format!(":{path}")])?;
    if !output.status.success() {
        return Err(format!(
            "git show failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    // Bun's Git wrapper decodes staged blobs with replacement characters.
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn git_stdout(repo: &Path, args: &[&str]) -> Result<String, String> {
    git_stdout_with_index(repo, None, args)
}

fn git_stdout_with_index(
    repo: &Path,
    index_file: Option<&Path>,
    args: &[&str],
) -> Result<String, String> {
    let output = git_output_with_index(repo, index_file, args)?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args[0],
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|error| format!("git {}: {error}", args[0]))
}

fn git_output(repo: &Path, args: &[&str]) -> Result<Output, String> {
    git_output_with_index(repo, None, args)
}

fn git_output_with_index(
    repo: &Path,
    index_file: Option<&Path>,
    args: &[&str],
) -> Result<Output, String> {
    let mut command = git(repo);
    if let Some(index) = index_file {
        let path = index;
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| format!("current directory: {error}"))?
                .join(path)
        };
        if !resolved.is_file() {
            return Err(format!(
                "explicit GIT_INDEX_FILE does not exist: {}",
                resolved.display()
            ));
        }
        command.env("GIT_INDEX_FILE", resolved);
    }
    command
        .args(args)
        .output()
        .map_err(|error| format!("git {}: {error}", args[0]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn git_ok(repo: &Path, args: &[&str]) {
        let out = git(repo).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn repo_with_index_declaration(declaration: Option<&str>) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, &["init", "-q"]);
        if let Some(text) = declaration {
            fs::write(repo.join(".develop-tools.json"), text).unwrap();
            git_ok(&repo, &["add", ".develop-tools.json"]);
        }
        (dir, repo)
    }

    #[test]
    fn declared_contract_is_read_from_the_index() {
        let (_dir, repo) = repo_with_index_declaration(Some(
            r#"{"integrationBranch":"develop","adrMigratedThrough":9,"adrSanitizedNumbers":[1,4,5]}"#,
        ));
        assert_eq!(
            adr_contract(&repo, None).unwrap(),
            AdrContract {
                migrated_through: Some(9),
                reserved_numbers: vec![],
                sanitized_numbers: vec![1, 4, 5],
            }
        );
    }

    #[test]
    fn index_declaration_wins_over_the_worktree() {
        let (_dir, repo) = repo_with_index_declaration(Some(r#"{"adrMigratedThrough":9}"#));
        fs::write(
            repo.join(".develop-tools.json"),
            r#"{"adrMigratedThrough":30}"#,
        )
        .unwrap();
        assert_eq!(adr_contract(&repo, None).unwrap().migrated_through, Some(9));
    }

    #[test]
    fn worktree_only_declaration_is_not_read() {
        let (_dir, repo) = repo_with_index_declaration(None);
        fs::write(
            repo.join(".develop-tools.json"),
            r#"{"adrMigratedThrough":9}"#,
        )
        .unwrap();
        assert!(adr_contract(&repo, None).is_err());
    }

    #[test]
    fn declaration_without_contract_keys_fails() {
        // 版の固定などだけを持つ宣言は、契約の宣言として扱わない。
        let (_dir, repo) = repo_with_index_declaration(Some(r#"{"integrationBranch":"develop"}"#));
        assert!(adr_contract(&repo, None).is_err());
    }

    #[test]
    fn broken_contract_declaration_fails() {
        let (_dir, repo) = repo_with_index_declaration(Some(r#"{"adrReservedNumbers":"20"}"#));
        assert!(adr_contract(&repo, None).is_err());
    }

    #[test]
    fn repo_without_declaration_fails() {
        let (_dir, repo) = repo_with_index_declaration(None);
        assert!(adr_contract(&repo, None).is_err());
    }

    #[test]
    fn list_staged_returns_only_cached_adr_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(repo.join("docs/adr")).unwrap();
        git_ok(&repo, &["init", "-q"]);
        git_ok(&repo, &["config", "user.email", "t@example.invalid"]);
        git_ok(&repo, &["config", "user.name", "t"]);
        git_ok(&repo, &["config", "commit.gpgsign", "false"]);
        fs::write(repo.join("docs/adr/0001-first.md"), "one\n").unwrap();
        fs::write(repo.join("src.txt"), "nope\n").unwrap();
        git_ok(&repo, &["add", "docs/adr/0001-first.md"]);
        let files = list_staged(&repo).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "0001-first.md");
        assert_eq!(files[0].content, "one\n");
    }

    #[test]
    fn staged_readme_uses_head_not_the_dirty_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(repo.join("docs/adr")).unwrap();
        git_ok(&repo, &["init", "-q"]);
        git_ok(&repo, &["config", "user.email", "t@example.invalid"]);
        git_ok(&repo, &["config", "user.name", "t"]);
        git_ok(&repo, &["config", "commit.gpgsign", "false"]);
        fs::write(repo.join("docs/adr/README.md"), "committed\n").unwrap();
        git_ok(&repo, &["add", "docs/adr/README.md"]);
        git_ok(&repo, &["commit", "-q", "-m", "readme"]);
        fs::write(repo.join("docs/adr/README.md"), "dirty worktree\n").unwrap();
        let readme = staged_readme(&repo, &[]).unwrap();
        assert_eq!(readme, "committed\n");
    }

    #[test]
    fn staged_readme_treats_a_deleted_readme_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(repo.join("docs/adr")).unwrap();
        git_ok(&repo, &["init", "-q"]);
        git_ok(&repo, &["config", "user.email", "t@example.invalid"]);
        git_ok(&repo, &["config", "user.name", "t"]);
        git_ok(&repo, &["config", "commit.gpgsign", "false"]);
        fs::write(
            repo.join("docs/adr/README.md"),
            "- [0001](./0001-first.md)\n",
        )
        .unwrap();
        git_ok(&repo, &["add", "docs/adr/README.md"]);
        git_ok(&repo, &["commit", "-q", "-m", "readme"]);
        git_ok(&repo, &["rm", "-q", "docs/adr/README.md"]);
        assert_eq!(staged_readme(&repo, &[]), None);
    }

    #[test]
    fn resolve_index_file_returns_same_path_for_relative_and_absolute() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, &["init", "-q"]);
        let git_dir = repo.join(".git");
        let custom_index = git_dir.join("custom_index");
        fs::write(&custom_index, b"").unwrap();

        let resolved_abs = resolve_index_file(&repo, &repo, &custom_index).unwrap();
        let rel_path = PathBuf::from(".git/custom_index");
        let resolved_rel = resolve_index_file(&repo, &repo, &rel_path).unwrap();

        assert_eq!(resolved_abs, resolved_rel);
        assert_eq!(resolved_abs, fs::canonicalize(&custom_index).unwrap());
    }

    #[test]
    fn resolve_index_file_rejects_path_outside_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, &["init", "-q"]);
        let outside_index = repo.join("outside_index");
        fs::write(&outside_index, b"").unwrap();

        let err = resolve_index_file(&repo, &repo, &outside_index).unwrap_err();
        assert!(err.contains("現在の worktree の Git directory 配下である必要があります"));
    }
}
