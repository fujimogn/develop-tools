//! repo の版数運用。
//!
//! **版数は「この機に入っている道具一式の版」を指す。** publish も deploy もしない repo では、
//! 版数の役目は配布物の同一性を言えることにある。開発機が複数あり、`bin/` の binary は
//! git 管理外なので、**どの機がどの版の道具を持っているかは版数でしか言えない**。
//!
//! 版 file は repo の `.develop-tools.json` に宣言する。

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub mod ci_runs;
pub mod preflight;
pub mod push;
pub mod release_notes;
pub mod version_files;

/// cwd が属する Git repo の root。repo 外だけ None とし、repo 内の Git 失敗は返す。
pub fn current_repo_root() -> Result<Option<PathBuf>, String> {
    let cwd = std::env::current_dir().map_err(|err| format!("cwd を読めません: {err}"))?;
    let out = develop_tools_core::git(&cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output();
    match out {
        Ok(out) if out.status.success() => {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            Ok(Some(PathBuf::from(path)))
        }
        Ok(_) | Err(_) if !cwd.ancestors().any(|dir| dir.join(".git").exists()) => Ok(None),
        Ok(out) => Err(format!(
            "git rev-parse --show-toplevel に失敗しました: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(err) => Err(format!("git rev-parse を起動できません: {err}")),
    }
}

/// 配布側の既存呼出点向け。版は `.develop-tools.json` の `release.versionFiles` だけから読む。
pub fn read_workspace_version(root: &Path) -> Result<Version, String> {
    Ok(version_files::VersionFiles::load(root)?.current())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseBinariesFile {
    binaries: Vec<String>,
}

/// 公開 CLI の一覧と非公開の一覧を合成する。
///
/// 公開側の file が無い Release 済み snapshot（v0.5.10 まで）は、その snapshot の
/// `config/release-binaries.json` だけで一覧を作る。移行の縮退。tasks 3.2 で外す。
pub fn load_release_binaries(root: &Path) -> Result<Vec<String>, String> {
    let public = root.join("packages/cli/config/release-binaries.json");
    let public_missing = match std::fs::metadata(&public) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => true,
        Err(err) => return Err(format!("{}: {err}", public.display())),
        Ok(_) => false,
    };
    let relatives: &[&str] = if public_missing {
        &["config/release-binaries.json"]
    } else {
        &[
            "packages/cli/config/release-binaries.json",
            "config/release-binaries.json",
        ]
    };
    let mut seen = std::collections::HashSet::new();
    let mut binaries = Vec::new();
    for relative in relatives {
        let path = root.join(relative);
        let text =
            std::fs::read_to_string(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        let file: ReleaseBinariesFile =
            serde_json::from_str(&text).map_err(|err| format!("{}: {err}", path.display()))?;
        if file.binaries.is_empty() {
            return Err(format!("{}: binary 一覧が空です", path.display()));
        }
        for name in file.binaries {
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
                || !seen.insert(name.clone())
            {
                return Err(format!(
                    "{}: 不正または重複した binary 名: {name}",
                    path.display()
                ));
            }
            binaries.push(name);
        }
    }
    Ok(binaries)
}

/// Release asset に入れる binary だけを tarball にする。
///
/// `cargo build --release --locked` の後、`tools/target/release` を `bin_dir` に渡す。
/// asset 名と checksum は release job が決める。
pub fn create_release_tarball(root: &Path, bin_dir: &Path, archive: &Path) -> Result<(), String> {
    let binaries = load_release_binaries(root)?;
    for name in &binaries {
        let path = bin_dir.join(name);
        if !path.is_file() {
            return Err(format!("配布対象 binary がありません: {}", path.display()));
        }
    }

    let output = std::process::Command::new("tar")
        .arg("-czf")
        .arg(archive)
        .arg("-C")
        .arg(bin_dir)
        .arg("--")
        .args(&binaries)
        .output()
        .map_err(|err| format!("tar を起動できません: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "tarball の作成に失敗しました: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// `x.y.z` の 3 つ組。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl std::str::FromStr for Version {
    type Err = String;

    /// **前ゼロ・余分な節を弾く。** `0.1.00` や `0.1.2.3` を通すと、書き戻した文字列と
    /// tag 名が食い違って guard がすり抜ける。
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = raw.split('.').collect();
        if parts.len() != 3 {
            return Err(format!("semver ではありません: {raw}"));
        }
        let mut nums = [0u64; 3];
        for (slot, part) in nums.iter_mut().zip(parts) {
            if part.is_empty() || (part.len() > 1 && part.starts_with('0')) {
                return Err(format!("semver ではありません: {raw}"));
            }
            *slot = part
                .parse()
                .map_err(|_| format!("semver ではありません: {raw}"))?;
        }
        Ok(Version {
            major: nums[0],
            minor: nums[1],
            patch: nums[2],
        })
    }
}

/// pre-push で送る release tag 1 件。`tag` は送り先の名前、`object` は local sha（annotated tag なら tag object）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushedTag {
    pub tag: String,
    pub object: String,
}

/// Git pre-push の stdin（`<local ref> <local sha> <remote ref> <remote sha>`）から、
/// 送り先が `refs/tags/v<semver>` の行だけを取り出す。**名前は remote ref で決める。**
/// `v1.2.3:refs/tags/v9.9.9` や `HEAD:refs/tags/v9.9.9` で作られるのは送り先の tag なので、
/// local ref で決めると別名の push で照合をすり抜ける。削除の行（local sha が全桁 0）と tag 以外の行は読み飛ばす。
pub fn pushed_release_tags(input: &str) -> Vec<PushedTag> {
    input
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (_local_ref, object, remote_ref) = (fields.next()?, fields.next()?, fields.next()?);
            let tag = remote_ref.strip_prefix("refs/tags/")?;
            if object.bytes().all(|b| b == b'0') {
                return None;
            }
            tag.strip_prefix('v')?.parse::<Version>().ok()?;
            Some(PushedTag {
                tag: tag.to_owned(),
                object: object.to_owned(),
            })
        })
        .collect()
}

/// 上げ方の指定。明示 semver か、段の指定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bump {
    Major,
    Minor,
    Patch,
    Exact(Version),
}

impl std::str::FromStr for Bump {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "major" => Ok(Bump::Major),
            "minor" => Ok(Bump::Minor),
            "patch" => Ok(Bump::Patch),
            other => other.parse::<Version>().map(Bump::Exact),
        }
    }
}

/// 次の版を出す。**明示指定は前へ戻せない。** 戻すと過去の tag と衝突し、
/// 「この版の道具」が 2 つ存在してしまう。
pub fn next_version(current: Version, bump: Bump) -> Result<Version, String> {
    let next = match bump {
        Bump::Major => Version {
            major: current.major + 1,
            minor: 0,
            patch: 0,
        },
        Bump::Minor => Version {
            major: current.major,
            minor: current.minor + 1,
            patch: 0,
        },
        Bump::Patch => Version {
            major: current.major,
            minor: current.minor,
            patch: current.patch + 1,
        },
        Bump::Exact(exact) => exact,
    };
    let ordered = |v: Version| (v.major, v.minor, v.patch);
    if ordered(next) <= ordered(current) {
        return Err(format!(
            "版を戻すか据え置く指定です ({current} → {next})。tag が衝突します"
        ));
    }
    Ok(next)
}

/// CHANGELOG のトップレベル項目の行頭と、太字の題の終わり。刻印を書く側と読む側で共有する。
pub(crate) const CHANGELOG_TITLE_PREFIX: &str = "- **";
pub(crate) const CHANGELOG_TITLE_END: &str = "**";
const CHANGELOG_UNRELEASED: &str = "、未リリース）";
/// 刻印の末尾（「（YYYY-MM-DD、v<版> で prod 反映）」の版より後ろ）。release notes はこれで刻印済みを見分ける。
pub(crate) const CHANGELOG_RELEASED_SUFFIX: &str = " で prod 反映）";
/// 刻印の版の前（「（YYYY-MM-DD、v<版> で prod 反映）」の日付と版の間）。
pub(crate) const CHANGELOG_RELEASED_VERSION_PREFIX: &str = "、v";

/// 題の末尾が島の標準の「（YYYY-MM-DD、未リリース）」なら、その「、未リリース）」の位置。
fn dated_unreleased_suffix(title: &str) -> Option<usize> {
    let head = title.strip_suffix(CHANGELOG_UNRELEASED)?;
    let date = head.get(head.len().checked_sub(10)?..)?;
    let is_date = date.bytes().enumerate().all(|(index, byte)| match index {
        4 | 7 => byte == b'-',
        _ => byte.is_ascii_digit(),
    });
    (is_date && head[..head.len() - 10].ends_with('（')).then_some(head.len())
}

/// CHANGELOG のトップレベル項目（行頭 `- **`）の太字の題の末尾「（YYYY-MM-DD、未リリース）」を
/// 「（YYYY-MM-DD、v<版> で prod 反映）」にし、書き換えた件数を返す。入れ子・本文・見出しと、
/// 日付の無い括弧は触らない。
pub fn stamp_changelog(text: &str, version: Version) -> (String, usize) {
    let mut count = 0;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let stamped = line.strip_prefix(CHANGELOG_TITLE_PREFIX).and_then(|rest| {
            let title_end = rest.find(CHANGELOG_TITLE_END)?;
            let suffix = dated_unreleased_suffix(&rest[..title_end])?;
            Some(format!(
                "{CHANGELOG_TITLE_PREFIX}{}{CHANGELOG_RELEASED_VERSION_PREFIX}{version}{CHANGELOG_RELEASED_SUFFIX}{}",
                &rest[..suffix],
                &rest[title_end..]
            ))
        });
        match stamped {
            Some(stamped) => {
                count += 1;
                out.push_str(&stamped);
            }
            None => out.push_str(line),
        }
    }
    (out, count)
}

/// JSON の top-level version の文字列値だけを探す。JSON の妥当性は呼出側が先に検証する。
pub(crate) fn package_version_span(text: &str) -> Result<std::ops::Range<usize>, String> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    let mut start = 0usize;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
                let is_version_key = depth == 1
                    && serde_json::from_str::<String>(&text[start - 1..=index])
                        .is_ok_and(|key| key == "version");
                if is_version_key {
                    let mut value = index + 1;
                    while bytes.get(value).is_some_and(u8::is_ascii_whitespace) {
                        value += 1;
                    }
                    if bytes.get(value) != Some(&b':') {
                        continue;
                    }
                    value += 1;
                    while bytes.get(value).is_some_and(u8::is_ascii_whitespace) {
                        value += 1;
                    }
                    if bytes.get(value) != Some(&b'"') {
                        return Err("package.json の version は文字列ではありません".to_owned());
                    }
                    let begin = value + 1;
                    value = begin;
                    while let Some(&next) = bytes.get(value) {
                        if next == b'"' && bytes.get(value.wrapping_sub(1)) != Some(&b'\\') {
                            return Ok(begin..value);
                        }
                        value += 1;
                    }
                }
            }
        } else {
            match byte {
                b'"' => {
                    quoted = true;
                    start = index + 1;
                }
                b'{' | b'[' => depth += 1,
                b'}' | b']' => depth -= 1,
                _ => {}
            }
        }
    }
    Err("package.json に top-level version がありません".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tools/Cargo.toml` があっても、宣言が無ければ版を読まない。
    #[test]
    fn workspace_version_without_declaration_does_not_read_tools_cargo() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("tools")).unwrap();
        std::fs::write(
            dir.path().join("tools/Cargo.toml"),
            "[workspace]\nmembers = []\n[workspace.package]\nversion = \"0.5.10\"\n",
        )
        .unwrap();
        assert!(
            read_workspace_version(dir.path())
                .unwrap_err()
                .contains(".develop-tools.json")
        );
        std::fs::write(
            dir.path().join(".develop-tools.json"),
            r#"{"integrationBranch":"develop"}"#,
        )
        .unwrap();
        assert!(
            read_workspace_version(dir.path())
                .unwrap_err()
                .contains("release")
        );
    }

    #[test]
    fn workspace_version_refuses_invalid_declaration() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("tools")).unwrap();
        std::fs::write(
            dir.path().join("tools/Cargo.toml"),
            "[workspace]\nmembers = []\n[workspace.package]\nversion = \"0.5.10\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join(".develop-tools.json"),
            r#"{"platforms":["linux"]}"#,
        )
        .unwrap();
        assert!(
            read_workspace_version(dir.path())
                .unwrap_err()
                .contains("preflightWorkflow")
        );
    }

    fn v(major: u64, minor: u64, patch: u64) -> Version {
        Version {
            major,
            minor,
            patch,
        }
    }

    #[test]
    fn a_version_string_round_trips() {
        assert_eq!("1.2.3".parse::<Version>().unwrap(), v(1, 2, 3));
        assert_eq!(v(1, 2, 3).to_string(), "1.2.3");
    }

    /// 前ゼロ・節の過不足を通すと、書き戻した文字列と tag 名が食い違って guard がすり抜ける。
    #[test]
    fn a_malformed_version_is_rejected() {
        for raw in ["0.1.00", "1.2", "1.2.3.4", "v1.2.3", "1.2.x", "", "1..3"] {
            assert!(raw.parse::<Version>().is_err(), "通ってはならない: {raw}");
        }
    }

    #[test]
    fn each_bump_moves_exactly_one_place() {
        let current = v(1, 2, 3);
        assert_eq!(next_version(current, Bump::Patch).unwrap(), v(1, 2, 4));
        assert_eq!(next_version(current, Bump::Minor).unwrap(), v(1, 3, 0));
        assert_eq!(next_version(current, Bump::Major).unwrap(), v(2, 0, 0));
    }

    /// 戻すと過去の tag と衝突し、同じ版の道具が 2 つ存在してしまう。
    #[test]
    fn a_backwards_or_equal_version_is_refused() {
        let current = v(1, 2, 3);
        for exact in [v(1, 2, 3), v(1, 2, 2), v(0, 9, 9)] {
            assert!(
                next_version(current, Bump::Exact(exact)).is_err(),
                "{exact}"
            );
        }
        assert_eq!(
            next_version(current, Bump::Exact(v(1, 2, 4))).unwrap(),
            v(1, 2, 4)
        );
    }
}
