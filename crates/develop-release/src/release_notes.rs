//! tag 間の CHANGELOG 月別項目から GitHub Release 本文を作る。

use std::path::Path;

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
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
        .map(|text| text.trim().to_owned())
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
    let marker_len = text
        .chars()
        .take_while(|ch| *ch == '`' || *ch == '~')
        .count();
    let marker = text.get(..marker_len)?;
    if marker_len < 3 || !marker.bytes().all(|b| b == marker.as_bytes()[0]) {
        return None;
    }
    Some((marker, &text[marker_len..]))
}

fn released_for_other_tag(item: &str, tag: &str) -> bool {
    let title = item
        .strip_prefix(crate::CHANGELOG_TITLE_PREFIX)
        .unwrap_or(item)
        .split(crate::CHANGELOG_TITLE_END)
        .next()
        .unwrap_or("");
    let Some(suffix) = title.strip_suffix(crate::CHANGELOG_RELEASED_SUFFIX) else {
        return false;
    };
    let Some(version) = suffix
        .rsplit_once(crate::CHANGELOG_RELEASED_VERSION_PREFIX)
        .map(|(_, version)| version)
    else {
        return false;
    };
    let marked = format!("v{version}");
    release_tag(&marked) && marked != tag
}

/// 全行の文脈を含む diff から、月別節に追加されたトップレベル項目だけを採る。
pub fn extract_changelog_additions(diff: &str, tag: &str) -> Vec<String> {
    let mut additions = Vec::new();
    let mut monthly = false;
    let mut fence: Option<String> = None;
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
        if let Some((marker, rest)) = fence_marker(content) {
            match &fence {
                None => fence = Some(marker.to_owned()),
                Some(open)
                    if marker.starts_with(&open[..1])
                        && marker.len() >= open.len()
                        && rest.trim().is_empty() =>
                {
                    fence = None
                }
                _ => {}
            }
            continue;
        }
        if fence.is_some() {
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

/// 不確実な tag や remote は本文を作らず失敗する。
pub fn release_notes(root: &Path, tag: &str, repo_arg: Option<&str>) -> Result<String, String> {
    if !release_tag(tag) {
        return Err(format!(
            "release tag は v<semver> で指定してください: {tag}"
        ));
    }
    let repo_root = git(root, &["rev-parse", "--show-toplevel"])?;
    let root = Path::new(&repo_root);
    let repo = match repo_arg {
        Some(repo) if valid_repo(repo) => repo.to_owned(),
        Some(repo) => return Err(format!("repo は owner/name で指定してください: {repo}")),
        None => remote_repo(root)?,
    };
    git(
        root,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/tags/{tag}^{{commit}}"),
        ],
    )?;
    let previous = git(
        root,
        &["describe", "--tags", "--abbrev=0", &format!("{tag}^")],
    )?;
    if !release_tag(&previous) {
        return Err(format!("前 tag を安全に決定できません: {previous}"));
    }
    let diff = git(
        root,
        &[
            "diff",
            "--unified=99999",
            &format!("{previous}..{tag}"),
            "--",
            "CHANGELOG.md",
        ],
    )?;
    let additions = extract_changelog_additions(&diff, tag);
    let compare = format!(
        "変更範囲: [{previous}...{tag}](https://github.com/{repo}/compare/{previous}...{tag})"
    );
    if additions.is_empty() {
        Ok(format!("{compare}\n"))
    } else {
        Ok(format!("{compare}\n\n{}\n", additions.join("\n")))
    }
}
