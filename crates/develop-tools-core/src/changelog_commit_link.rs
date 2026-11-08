//! Pure parsing of optional CHANGELOG commit-link suffixes; no Git evidence.

use std::ops::Range;

const SHA_LENGTH: Range<usize> = 7..41;
const URL_PREFIX: &str = "https://github.com/";
const COMMIT_PATH: &str = "/commit/";

/// Syntax only: the repository and commit have not been checked against Git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitLink<'a> {
    pub sha: &'a str,
    pub owner: &'a str,
    pub repo: &'a str,
    /// Byte range in the original item, including the single preceding space.
    pub suffix_range: Range<usize>,
}

/// A commit-link candidate exists but is not a single canonical first-line suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MalformedCommitLink;

/// Inspect only the first-line suffix; other links are ordinary references.
pub fn parse(item: &str) -> Result<Option<CommitLink<'_>>, MalformedCommitLink> {
    let first_line = item
        .split('\n')
        .next()
        .unwrap_or_default()
        .trim_end_matches('\r');
    let suffix_end = first_line.trim_end_matches([' ', '\t']).len();
    let masked = visible_bytes(item);
    let visible = &masked[..first_line.len()];
    let mut candidate = None;
    let mut opening = None;
    let mut index = 0;
    while index < visible.len() {
        match visible[index] {
            b'[' => opening = Some(index),
            b'\n' => opening = None,
            b']' if visible.get(index + 1) == Some(&b'(') => {
                let end = visible[index + 2..]
                    .iter()
                    .position(|b| matches!(b, b')' | b'\n'))
                    .map_or(visible.len(), |at| index + 2 + at);
                let label = opening.map(|at| &visible[at + 1..index]);
                let target = &visible[index + 2..end];
                let is_candidate = target
                    .windows(COMMIT_PATH.len())
                    .any(|w| w == COMMIT_PATH.as_bytes())
                    || label.is_some_and(|label| {
                        label.iter().all(u8::is_ascii_hexdigit)
                            && label.iter().any(u8::is_ascii_alphabetic)
                    });
                let link_end = end + usize::from(visible.get(end) == Some(&b')'));
                if is_candidate && link_end == suffix_end {
                    let start = opening.ok_or(MalformedCommitLink)?;
                    candidate = Some(start..link_end);
                }
                opening = None;
                index = end;
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    let Some(range) = candidate else {
        return Ok(None);
    };
    let start = range.start;
    if range.end != first_line.len()
        || start == 0
        || item.as_bytes()[start - 1] != b' '
        || (start > 1 && item.as_bytes()[start - 2].is_ascii_whitespace())
    {
        return Err(MalformedCommitLink);
    }
    let raw = &item[range.clone()];
    // Masked code must never become part of an otherwise canonical link.
    if raw.as_bytes() != &visible[range.clone()] {
        return Err(MalformedCommitLink);
    }
    let (label, url) = raw
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(')'))
        .and_then(|s| s.split_once("]("))
        .ok_or(MalformedCommitLink)?;
    let path = url.strip_prefix(URL_PREFIX).ok_or(MalformedCommitLink)?;
    let (repository, sha) = path.split_once(COMMIT_PATH).ok_or(MalformedCommitLink)?;
    let (owner, repo) = repository.split_once('/').ok_or(MalformedCommitLink)?;
    if !valid_sha(label)
        || !valid_sha(sha)
        || label != sha
        || !valid_name(owner)
        || !valid_name(repo)
        || repo.ends_with(".git")
    {
        return Err(MalformedCommitLink);
    }
    Ok(Some(CommitLink {
        sha,
        owner,
        repo,
        suffix_range: start - 1..range.end,
    }))
}

fn valid_sha(value: &str) -> bool {
    SHA_LENGTH.contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

/// Keep byte offsets stable while ignoring fences and matched backtick spans.
/// Fence rules match island-docs: up to three spaces, same marker, closing run
/// at least as long as the opener and followed only by whitespace.
fn visible_bytes(item: &str) -> Vec<u8> {
    let mut visible = item.as_bytes().to_vec();
    let mut fence = None;
    let mut offset = 0;
    for line in item.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let indent = body.len() - body.trim_start_matches(' ').len();
        let rest = &body[indent..];
        let marker = rest.as_bytes().first().copied().unwrap_or_default();
        let length = rest.bytes().take_while(|b| *b == marker).count();
        let is_marker = indent <= 3 && matches!(marker, b'`' | b'~') && length >= 3;
        let was_fenced = fence.is_some();
        match fence {
            Some((open, run))
                if is_marker
                    && marker == open
                    && length >= run
                    && rest[length..].trim().is_empty() =>
            {
                fence = None
            }
            None if is_marker && (marker != b'`' || !rest[length..].contains('`')) => {
                fence = Some((marker, length))
            }
            _ => {}
        }
        if was_fenced || fence.is_some() {
            visible[offset..offset + body.len()].fill(b' ');
        }
        offset += line.len();
    }
    let mut index = 0;
    while index < visible.len() {
        if visible[index] != b'`' {
            index += 1;
            continue;
        }
        let run = visible[index..].iter().take_while(|b| **b == b'`').count();
        let mut next = index + run;
        let mut close = None;
        while next < visible.len() {
            if visible[next] == b'`' {
                let length = visible[next..].iter().take_while(|b| **b == b'`').count();
                if length == run {
                    close = Some(next + length);
                    break;
                }
                next += length;
            } else {
                next += 1;
            }
        }
        if let Some(end) = close {
            visible[index..end].fill(b' ');
            index = end;
        } else {
            index += run;
        }
    }
    visible
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(sha: &str) -> String {
        format!("[{sha}](https://github.com/owner/repo/commit/{sha})")
    }

    #[test]
    fn absent_and_ordinary_links_are_none() {
        for item in [
            "",
            "- 成果。",
            "- [ADR](docs/adr/0035.md)",
            "- [issue](https://github.com/a/b/issues/1)",
        ] {
            assert_eq!(parse(item), Ok(None), "{item}");
        }
    }

    #[test]
    fn references_outside_the_suffix_and_numeric_labels_are_ordinary() {
        let valid = link("b0fa456d");
        for item in [
            "- ADR [0033](docs/adr/0033-example.md) を記録。".to_owned(),
            "- ADR [0033](docs/adr/0033-example.md)".to_owned(),
            "- run [40705334273](https://github.com/owner/repo/actions/runs/40705334273) を観測。"
                .to_owned(),
            "- run [40705334273](https://github.com/owner/repo/actions/runs/40705334273)"
                .to_owned(),
            format!("- 成果。{valid}、[観測評価](docs/evaluation.md)）。"),
            format!("- 成果。 {valid} 注記"),
            format!("- 成果。 {valid}。"),
            format!("- 成果。\n  {valid}"),
        ] {
            assert_eq!(parse(&item), Ok(None), "{item}");
        }
        let item = format!("- 参照 {valid} 成果。 {}", link("c16d9a5"));
        assert_eq!(parse(&item).unwrap().unwrap().sha, "c16d9a5");
    }

    #[test]
    fn canonical_suffix_keeps_borrowed_identity_and_original_byte_range() {
        for sha in [
            "c16d9a5",
            "1234567",
            "abcdefa",
            "0123456789abcdef0123456789abcdef01234567",
        ] {
            for ending in ["", "\n", "\r\n"] {
                let suffix = format!(" {}", link(sha));
                let item = format!("- **成果**: 日本語。{suffix}{ending}");
                let parsed = parse(&item).unwrap().expect("valid suffix");
                assert_eq!(parsed.sha, sha);
                assert_eq!((parsed.owner, parsed.repo), ("owner", "repo"));
                assert_eq!(&item[parsed.suffix_range], suffix);
            }
        }
        let item = format!("- 成果。 {}\n  継続文。", link("c16d9a5"));
        assert!(parse(&item).unwrap().is_some());
    }

    #[test]
    fn malformed_candidates_do_not_disappear_as_absent() {
        let valid = link("c16d9a5");
        for item in [
            format!("- 成果。 {}", valid.replace("[c16d9a5]", "[c16d9a6]")),
            format!("- 成果。 {}", link("abcdef")),
            format!("- 成果。 {}", link(&"a".repeat(41))),
            format!("- 成果。 {}", link(&"a".repeat(64))),
            format!("- 成果。 {}", link("C16D9A5")),
            format!(
                "- 成果。 {}",
                valid.replace("https://github.com", "https://example.com")
            ),
            "- 成果。 [c16d9a5](/commit/c16d9a5)".into(),
            "- 成果。 [c16d9a5](docs/adr.md)".into(),
            "- 成果。 [commit](https://github.com/a/b/commit/c16d9a5".into(),
            "- 成果。 ](https://github.com/a/b/commit/c16d9a5".into(),
            format!("- 成果。{valid}"),
            format!("- 成果。  {valid}"),
            format!("- 成果。\t{valid}"),
            format!("- 成果。 {valid} "),
            format!("- 成果。 {valid}\t"),
        ] {
            assert_eq!(parse(&item), Err(MalformedCommitLink), "{item}");
        }
        for tail in ["?x=1", "#x", ".git", "/extra"] {
            let item = format!(
                "- 成果。 {}",
                valid.replace("c16d9a5)", &format!("c16d9a5{tail})"))
            );
            assert_eq!(parse(&item), Err(MalformedCommitLink), "{item}");
        }
        for url in [
            "http://github.com/a/b/commit/c16d9a5",
            "https://user@github.com/a/b/commit/c16d9a5",
            "https://github.com:443/a/b/commit/c16d9a5",
            "https://github.com/a/b.git/commit/c16d9a5",
            "https://github.com/a/b%20c/commit/c16d9a5",
        ] {
            assert_eq!(
                parse(&format!("- 成果。 [c16d9a5]({url})")),
                Err(MalformedCommitLink)
            );
        }
    }

    #[test]
    fn code_spans_and_fences_are_not_candidates() {
        let valid = link("c16d9a5");
        for item in [
            format!("- 例: `{valid}`"),
            format!("- 例: `` `{valid}` ``"),
            format!("- 成果。\n```md\n{valid}\n```"),
            format!("- 成果。\n  ~~~~md\n{valid}\n  ~~~~~"),
            format!("- 成果。\n```md\n{valid}"),
        ] {
            assert_eq!(parse(&item), Ok(None), "{item}");
        }
        let item = format!("- 例: `{valid}` 成果。 {valid}");
        assert!(parse(&item).unwrap().is_some());
    }

    #[test]
    fn code_before_suffix_preserves_the_original_separator() {
        let item = format!("- `例` {}", link("c16d9a5"));
        assert!(parse(&item).unwrap().is_some());
    }

    #[test]
    fn masked_backtick_cannot_replace_the_required_space() {
        let item = format!("- 成果。`x`{}", link("c16d9a5"));
        assert_eq!(parse(&item), Err(MalformedCommitLink));
    }

    #[test]
    fn full_sha_and_repository_name_characters_are_preserved() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(sha.len(), 40);
        let item = format!("- 成果。 [{sha}](https://github.com/Owner_1/Repo.name-2/commit/{sha})");
        let parsed = parse(&item).unwrap().unwrap();
        assert_eq!(parsed.sha, sha);
        assert_eq!((parsed.owner, parsed.repo), ("Owner_1", "Repo.name-2"));
    }
}
