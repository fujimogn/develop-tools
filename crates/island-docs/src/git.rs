//! git の index から tracked の path と本文を読む（design §5.1 の tracked 基準）。
//!
//! git の起動は [`develop_core::git`] を通し、継いだ `GIT_*` を落とす。
//! pre-commit が使う一時 index は `--index-file` で明示されたときだけ `GIT_INDEX_FILE` に入れる
//! （`bin/check-staged-docs.ts` と同じ扱い）。

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 検査する repo（git の toplevel）。
#[derive(Debug)]
pub struct Repo {
    root: PathBuf,
    index_file: Option<PathBuf>,
}

/// index の 1 entry。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub mode: String,
    pub oid: String,
    pub path: String,
}

impl Entry {
    /// 本文を読める通常 file か（symlink と submodule は本文を読まない）。
    pub fn is_regular_file(&self) -> bool {
        self.mode == "100644" || self.mode == "100755"
    }
}

impl Repo {
    /// `cwd` を含む repo を開く。`index_file` は現在 worktree の git dir の下の通常 file に限る。
    pub fn discover(cwd: &Path, index_file: Option<&Path>) -> Result<Self, String> {
        let root = PathBuf::from(output_text(
            develop_core::git(cwd).args(["rev-parse", "--show-toplevel"]),
        )?);
        let index_file = match index_file {
            None => None,
            Some(path) => Some(validate_index_file(&root, path)?),
        };
        Ok(Self { root, index_file })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn git(&self) -> Command {
        let mut command = develop_core::git(&self.root);
        if let Some(index) = &self.index_file {
            command.env("GIT_INDEX_FILE", index);
        }
        command
    }

    /// `remote.origin.url` から GitHub の `owner/name` を引く。無ければ None。
    pub fn github_name(&self) -> Option<String> {
        let url = output_text(self.git().args(["config", "--get", "remote.origin.url"])).ok()?;
        github_name_from_url(&url)
    }

    /// index の entry。衝突中の同じ path は 1 つにまとめる。
    pub fn index_entries(&self) -> Result<Vec<Entry>, String> {
        let raw = output_bytes(self.git().args(["ls-files", "--stage", "-z"]))?;
        let mut seen = BTreeSet::new();
        let mut entries = Vec::new();
        for record in raw.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let record = String::from_utf8_lossy(record);
            let Some((meta, path)) = record.split_once('\t') else {
                return Err(format!("git ls-files の出力を読めません: {record}"));
            };
            let mut fields = meta.split(' ');
            let (Some(mode), Some(oid)) = (fields.next(), fields.next()) else {
                return Err(format!("git ls-files の出力を読めません: {record}"));
            };
            if seen.insert(path.to_owned()) {
                entries.push(Entry {
                    mode: mode.to_owned(),
                    oid: oid.to_owned(),
                    path: path.to_owned(),
                });
            }
        }
        Ok(entries)
    }

    /// head commit の tree にある通常 file と path。index や worktree の状態に依存しない。
    pub fn tree_entries(&self, head: &str) -> Result<Vec<Entry>, String> {
        let head = self.commit_oid(head, "head")?;
        let raw = output_bytes(
            self.git()
                .args(["ls-tree", "-r", "--full-tree", "-z", &head]),
        )?;
        let mut entries = Vec::new();
        for record in raw.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let record = String::from_utf8_lossy(record);
            let Some((meta, path)) = record.split_once('\t') else {
                return Err(format!("git ls-tree の出力を読めません: {record}"));
            };
            let mut fields = meta.split(' ');
            let (Some(mode), Some(_kind), Some(oid)) =
                (fields.next(), fields.next(), fields.next())
            else {
                return Err(format!("git ls-tree の出力を読めません: {record}"));
            };
            entries.push(Entry {
                mode: mode.to_owned(),
                oid: oid.to_owned(),
                path: path.to_owned(),
            });
        }
        Ok(entries)
    }

    /// base..head の変更 path。base が初回 push の全 0、または取得不能なら最後の commit だけを見る。
    pub fn range_changes(&self, base: &str, head: &str) -> Result<BTreeSet<String>, String> {
        let head = self.commit_oid(head, "head")?;
        if !valid_oid(base) {
            return Err("base は 40 桁または 64 桁の commit ID にしてください".to_owned());
        }
        let base = if base.bytes().all(|byte| byte == b'0') {
            None
        } else {
            self.resolve_commit(base)
        };
        let raw = if let Some(base) = base {
            output_bytes(self.git().args([
                "diff",
                "--name-only",
                "--no-renames",
                "-z",
                &base,
                &head,
            ]))?
        } else if let Some(parent) = self.resolve_commit(&format!("{head}^")) {
            output_bytes(self.git().args([
                "diff",
                "--name-only",
                "--no-renames",
                "-z",
                &parent,
                &head,
            ]))?
        } else {
            output_bytes(self.git().args([
                "diff-tree",
                "--root",
                "--no-commit-id",
                "--name-only",
                "--no-renames",
                "-r",
                "-z",
                &head,
            ]))?
        };
        Ok(raw
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect())
    }

    fn commit_oid(&self, oid: &str, label: &str) -> Result<String, String> {
        if !valid_oid(oid) {
            return Err(format!(
                "{label} は 40 桁または 64 桁の commit ID にしてください"
            ));
        }
        self.resolve_commit(oid)
            .ok_or_else(|| format!("{label} commit を読めません: {oid}"))
    }

    fn resolve_commit(&self, rev: &str) -> Option<String> {
        output_text(self.git().args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ]))
        .ok()
    }

    /// blob の本文を `oids` と同じ順に返す。1 回の `git cat-file --batch` で読む。
    pub fn read_blobs(&self, oids: &[&str]) -> Result<Vec<Vec<u8>>, String> {
        if oids.is_empty() {
            return Ok(Vec::new());
        }
        let mut child = self
            .git()
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("git cat-file を起動できません: {error}"))?;
        let input: String = oids.iter().map(|oid| format!("{oid}\n")).collect();
        let mut stdin = child.stdin.take().expect("piped stdin");
        // 出力を読む前に全部書くと pipe が詰まるので、書き込みは別 thread に出す。
        let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
        let mut raw = Vec::new();
        child
            .stdout
            .take()
            .expect("piped stdout")
            .read_to_end(&mut raw)
            .map_err(|error| format!("git cat-file の出力を読めません: {error}"))?;
        let status = child
            .wait()
            .map_err(|error| format!("git cat-file が終わりません: {error}"))?;
        writer
            .join()
            .expect("writer thread")
            .map_err(|error| format!("git cat-file へ書けません: {error}"))?;
        if !status.success() {
            return Err("git cat-file --batch が失敗しました".to_owned());
        }
        parse_batch(&raw, oids)
    }

    /// staged で変わった path（追加・変更・削除。rename は削除と追加の 2 つに分ける）。
    pub fn staged_changes(&self) -> Result<BTreeSet<String>, String> {
        let raw = output_bytes(self.git().args([
            "diff",
            "--cached",
            "--name-only",
            "--no-renames",
            "-z",
        ]))?;
        Ok(raw
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect())
    }
}

fn valid_oid(oid: &str) -> bool {
    (oid.len() == 40 || oid.len() == 64) && oid.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_index_file(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let resolved = std::fs::canonicalize(path)
        .map_err(|error| format!("明示 index を読み取れません: {}: {error}", path.display()))?;
    if !resolved.is_file() {
        return Err(format!(
            "明示 index は通常 file である必要があります: {}",
            path.display()
        ));
    }
    let git_dir = output_text(develop_core::git(root).args(["rev-parse", "--absolute-git-dir"]))?;
    let git_dir = std::fs::canonicalize(&git_dir)
        .map_err(|error| format!("git dir を確認できません: {git_dir}: {error}"))?;
    if !resolved.starts_with(&git_dir) {
        return Err(format!(
            "明示 index は現在 worktree の git dir の下にある必要があります: {}",
            path.display()
        ));
    }
    Ok(resolved)
}

fn parse_batch(raw: &[u8], oids: &[&str]) -> Result<Vec<Vec<u8>>, String> {
    let mut blobs = Vec::with_capacity(oids.len());
    let mut offset = 0;
    for oid in oids {
        let header_end = raw[offset..]
            .iter()
            .position(|b| *b == b'\n')
            .map(|at| offset + at)
            .ok_or_else(|| format!("git cat-file の header がありません: {oid}"))?;
        let header = String::from_utf8_lossy(&raw[offset..header_end]);
        let size = header
            .strip_prefix(&format!("{oid} blob "))
            .and_then(|size| size.parse::<usize>().ok())
            .ok_or_else(|| format!("git cat-file が blob を返しません: {header}"))?;
        let start = header_end + 1;
        let end = start + size;
        if raw.get(end) != Some(&b'\n') {
            return Err(format!("git cat-file の本文が途切れています: {oid}"));
        }
        blobs.push(raw[start..end].to_vec());
        offset = end + 1;
    }
    Ok(blobs)
}

/// `https://github.com/o/n(.git)`、`git@github.com:o/n(.git)`、`ssh://git@github.com/o/n` から `o/n`。
pub fn github_name_from_url(url: &str) -> Option<String> {
    develop_core::github_repo(url)
}

fn output_bytes(command: &mut Command) -> Result<Vec<u8>, String> {
    let out = command
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("git を起動できません: {error}"))?;
    if !out.status.success() {
        return Err(format!(
            "git が失敗しました: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

fn output_text(command: &mut Command) -> Result<String, String> {
    let bytes = output_bytes(command)?;
    Ok(String::from_utf8_lossy(&bytes).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_names_from_remote_urls() {
        for url in [
            "https://github.com/owner/sample.git",
            "https://github.com/owner/sample",
            "git@github.com:owner/sample.git",
            "ssh://git@github.com/owner/sample.git",
            "https://x-access-token:t@github.com/owner/sample/",
        ] {
            assert_eq!(
                github_name_from_url(url).as_deref(),
                Some("owner/sample"),
                "{url}"
            );
        }
        assert_eq!(github_name_from_url("https://example.com/a/b"), None);
        assert_eq!(
            github_name_from_url("https://notgithub.com/owner/repo"),
            None
        );
        let token = github_name_from_url("https://x-access-token:secret@github.com/o/n")
            .expect("token url");
        assert_eq!(token, "o/n");
        assert!(!token.contains("secret"));
        assert_eq!(
            github_name_from_url("https://github.com/owner/repo/extra"),
            None
        );
    }

    #[test]
    fn batch_output_is_split_by_declared_size() {
        let raw = b"aa blob 3\nx\ny\nbb blob 0\n\n";
        assert_eq!(
            parse_batch(raw, &["aa", "bb"]).expect("parse"),
            [b"x\ny".to_vec(), Vec::new()]
        );
        assert!(parse_batch(b"aa missing\n", &["aa"]).is_err());
    }
}
