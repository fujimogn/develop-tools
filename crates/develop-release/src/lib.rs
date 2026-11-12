//! repo の版数運用。
//!
//! **版数は「この機に入っている道具一式の版」を指す。** publish も deploy もしない repo では、
//! 版数の役目は配布物の同一性を言えることにある。開発機が複数あり、`bin/` の binary は
//! git 管理外なので、**どの機がどの版の道具を持っているかは版数でしか言えない**。
//!
//! 版 file は repo の版固定 file（`.config/develop-tools.json`、移行中は旧い `.develop-tools.json`）に宣言する。

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub mod ai_check;
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

/// 配布側の既存呼出点向け。版は版固定 file の `release.versionFiles` だけから読む。
pub fn read_workspace_version(root: &Path) -> Result<Version, String> {
    Ok(version_files::VersionFiles::load(root)?.current())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseBinariesFile {
    binaries: Vec<String>,
    #[serde(rename = "includePublic", default = "include_public_by_default")]
    include_public: bool,
}

fn include_public_by_default() -> bool {
    true
}

fn read_release_binaries_file(path: &Path) -> Result<ReleaseBinariesFile, String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    serde_json::from_str(&text).map_err(|err| format!("{}: {err}", path.display()))
}

/// snapshot の宣言に従って公開一覧と専用一覧を合成する。
///
/// `includePublic` の省略/true は従来の一覧を保ち、false は専用一覧だけを使う。
/// 公開 manifest の無い旧 snapshot では、その snapshot の専用一覧を使う。
pub fn load_release_binaries(root: &Path) -> Result<Vec<String>, String> {
    let private_path = root.join("config/release-binaries.json");
    let private = read_release_binaries_file(&private_path)?;
    let public_path = root.join("packages/cli/config/release-binaries.json");
    let public = if private.include_public {
        match std::fs::metadata(&public_path) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(format!("{}: {err}", public_path.display())),
            Ok(_) => Some(read_release_binaries_file(&public_path)?),
        }
    } else {
        None
    };
    let mut seen = std::collections::HashSet::new();
    let mut binaries = Vec::new();
    for (path, file) in public
        .into_iter()
        .map(|file| (&public_path, file))
        .chain(std::iter::once((&private_path, private)))
    {
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

    /// **前ゼロ・数字以外（`+1` のような符号を含む）・余分な節を弾く。** `0.1.00` や `0.1.2.3` を
    /// 通すと、書き戻した文字列と tag 名が食い違って guard がすり抜ける。
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = raw.split('.').collect();
        if parts.len() != 3 {
            return Err(format!("semver ではありません: {raw}"));
        }
        let mut nums = [0u64; 3];
        for (slot, part) in nums.iter_mut().zip(parts) {
            if part.is_empty()
                || !part.bytes().all(|byte| byte.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
            {
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

/// 月別 CHANGELOG のトップレベル項目の行頭。
pub(crate) const CHANGELOG_TITLE_PREFIX: &str = "- **";

/// 日付の文字列が `YYYY-MM-DD` 形式かを返す。
pub(crate) fn is_date(text: &str) -> bool {
    text.len() == 10
        && text.bytes().enumerate().all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

/// トップレベル項目の未リリース印を Release link へ書き換える。
/// 日付・本文・改行は保ち、入れ子・見出し・code fence の中は触らない。
pub fn stamp_changelog(text: &str, version: Version, repo: &str) -> (String, usize) {
    stamp_changelog_with_language(
        text,
        version,
        repo,
        develop_tools_core::develop_tools_manifest::NotesLanguage::Ja,
    )
}

/// 宣言した言語の未公開印だけを置換する。
pub fn stamp_changelog_with_language(
    text: &str,
    version: Version,
    repo: &str,
    language: develop_tools_core::develop_tools_manifest::NotesLanguage,
) -> (String, usize) {
    use develop_tools_core::changelog_title::{self, Stamp};

    let mut count = 0;
    let mut out = String::with_capacity(text.len());
    let mut fence = release_notes::Fence::default();
    for line in text.split_inclusive('\n') {
        let parsed = if fence.step(line) || fence.is_open() {
            None
        } else {
            changelog_title::parse_with_language(line, language)
                .filter(|parsed| parsed.stamp == Stamp::Unreleased)
        };
        if let Some(parsed) = parsed {
            out.push_str(&line[..parsed.stamp_range.start]);
            out.push_str(&format!(
                "/[v{version}](https://github.com/{repo}/releases/tag/v{version})"
            ));
            out.push_str(&line[parsed.stamp_range.end..]);
            count += 1;
            continue;
        }
        out.push_str(line);
    }
    (out, count)
}

/// 事前照合と実書込みで、同じ宣言から同じ移動・刻印の結果を作る。
pub(crate) fn stamp_changelog_for_notes(
    text: &str,
    version: Version,
    repo: &str,
    notes: Option<release_notes::DeclaredNotes>,
) -> Result<(String, usize), String> {
    use develop_tools_core::develop_tools_manifest::{NotesLanguage, NotesSource};
    let language = notes.map_or(NotesLanguage::Ja, |notes| notes.language);
    let planned = notes
        .filter(|notes| notes.source == NotesSource::ChangelogSection)
        .map(|_| prepare_changelog_section(text, version, language))
        .transpose()?;
    Ok(stamp_changelog_with_language(
        planned.as_deref().unwrap_or(text),
        version,
        repo,
        language,
    ))
}

/// 月直下の未公開項目を、作成側が用意した今回版の要約の後へ移す。
/// 旧 ## 版の節は変更せず、書込み前に全対象の所属を確かめる。
pub(crate) fn prepare_changelog_section(
    text: &str,
    version: Version,
    language: develop_tools_core::develop_tools_manifest::NotesLanguage,
) -> Result<String, String> {
    use develop_tools_core::changelog_title::{Stamp, parse_version_heading, parse_with_language};

    let expected = version.to_string();
    let mut month = None;
    let mut saw_month = false;
    let mut in_version = false;
    let mut target = None;
    let mut target_end = None;
    let mut pending = Vec::new();
    let mut item: Option<(String, String, usize)> = None;
    let mut fence = release_notes::Fence::default();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        // 空行は項目と一緒に保ち、独立した段落や fence は元の位置へ残す。
        if !fence.is_open()
            && !line.trim().is_empty()
            && !line.starts_with([' ', '\t'])
            && let Some((title, date, start)) = item.take()
        {
            pending.push((title, date, start..offset));
        }
        let fenced = fence.step(line) || fence.is_open();
        if !fenced {
            let heading =
                line.starts_with("# ") || line.starts_with("## ") || line.starts_with("### ");
            if target.is_some() && target_end.is_none() && heading {
                target_end = Some(offset);
            }
            let plain = line.trim_end_matches(['\r', '\n']);
            if line.starts_with("# ") || line.starts_with("## ") {
                month = release_notes::monthly_heading(plain).then(|| plain[3..].to_owned());
                saw_month |= month.is_some();
                in_version = false;
            } else if let Some((found, date)) = parse_version_heading(plain) {
                in_version = true;
                if found == expected {
                    if target.is_some() {
                        return Err(
                            "changelog_section_duplicate: 今回版の小節が複数あります".to_owned()
                        );
                    }
                    if month.as_deref() != date.get(..7) {
                        return Err(
                            "changelog_section_month: 今回版の日付と月見出しが一致しません"
                                .to_owned(),
                        );
                    }
                    target = Some(date.to_owned());
                    target_end = None;
                }
            }
            if let Some(parsed) = parse_with_language(line, language)
                && parsed.stamp == Stamp::Unreleased
                && let Some(month) = &month
            {
                if in_version {
                    return Err(format!(
                        "changelog_section_pending: {} ({month}) は版小節内にあります。未公開項目は月の直下に置いてください",
                        parsed.title
                    ));
                }
                item = Some((parsed.title.to_owned(), parsed.date.to_owned(), offset));
            }
        }
        offset += line.len();
    }
    if let Some((title, date, start)) = item {
        pending.push((title, date, start..text.len()));
    }
    let Some(target_date) = target else {
        // 旧版節は従来の読取・刻印契約を維持する。
        if saw_month {
            return Err(format!(
                "changelog_section_missing: ### {version} (YYYY-MM-DD) と要約を用意してください"
            ));
        }
        return Ok(text.to_owned());
    };
    let section = release_notes::extract_changelog_section(text, &expected)
        .map_err(|error| format!("changelog_section_invalid: {error}"))?;
    if section.body.starts_with("- ") {
        return Err(
            "changelog_section_summary_missing: 今回版小節の直下に要約段落が必要です".to_owned(),
        );
    }
    let future_items: Vec<_> = pending
        .iter()
        .filter(|(_, date, _)| date > &target_date)
        .map(|(title, date, _)| format!("{title} ({date})"))
        .collect();
    if !future_items.is_empty() {
        return Err(format!(
            "changelog_section_date: {} は今回版の日付 {target_date} より後です",
            future_items.join(", ")
        ));
    }
    if pending.is_empty() {
        return Err("changelog_section_pending_empty: 月直下に未公開項目がありません".to_owned());
    }
    let insert = target_end.unwrap_or(text.len());
    let mut moved = String::new();
    let mut edits = Vec::with_capacity(pending.len() + 1);
    for (_, _, range) in pending {
        moved.push_str(&text[range.clone()]);
        edits.push((range, String::new()));
    }
    let separator = if text[..insert].ends_with("\n\n") {
        ""
    } else if text[..insert].ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    moved.insert_str(0, separator);
    if insert < text.len() && !moved.ends_with('\n') {
        moved.push('\n');
    }
    edits.push((insert..insert, moved));
    edits.sort_by_key(|(range, _)| range.start);
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (range, replacement) in edits {
        out.push_str(&text[cursor..range.start]);
        out.push_str(&replacement);
        cursor = range.end;
    }
    out.push_str(&text[cursor..]);
    Ok(out)
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
                .contains(develop_tools_core::develop_tools_manifest::PATH)
        );
        std::fs::create_dir_all(dir.path().join(".config")).unwrap();
        std::fs::write(
            dir.path().join(".config/develop-tools.json"),
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
        std::fs::create_dir_all(dir.path().join(".config")).unwrap();
        std::fs::write(
            dir.path().join(".config/develop-tools.json"),
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
        for raw in [
            "0.1.00", "1.2", "1.2.3.4", "v1.2.3", "1.2.x", "", "1..3", "+1.2.3", "1.+2.3",
        ] {
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
