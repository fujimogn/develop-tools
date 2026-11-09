//! Release repo の CHANGELOG の括弧内刻印。
use std::ops::Range;

pub const UNRELEASED_STAMP: &str = "/⏳";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stamp<'a> {
    Unreleased,
    Released(&'a str),
}

#[derive(Debug, PartialEq, Eq)]
pub struct StampedTitle<'a> {
    pub title: &'a str,
    pub date: &'a str,
    pub stamp: Stamp<'a>,
    pub release_url: Option<&'a str>,
    pub supplement: Option<&'a str>,
    pub summary: &'a str,
    pub metadata_range: Range<usize>,
    pub stamp_range: Range<usize>,
}

fn version(raw: &str) -> Option<&str> {
    let mut parts = raw.split('.');
    for _ in 0..3 {
        let part = parts.next()?;
        if part.is_empty()
            || (part.len() > 1 && part.starts_with('0'))
            || !part.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
    }
    parts.next().is_none().then_some(raw)
}

fn github_release_url(url: &str, tag: &str) -> bool {
    let Some(path) = url.strip_prefix("https://github.com/") else {
        return false;
    };
    let Some((repo, actual_tag)) = path.split_once("/releases/tag/") else {
        return false;
    };
    if actual_tag != tag {
        return false;
    }
    let mut names = repo.split('/');
    let valid = |name: &str| {
        !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    matches!((names.next(), names.next(), names.next()), (Some(owner), Some(name), None) if valid(owner) && valid(name))
}

/// `- **` の後から、題の終わりになる最初の `**` を見つける。
pub fn title_closing_marker(rest: &str) -> Option<usize> {
    rest.match_indices("**").find_map(|(index, _)| {
        let after = &rest[index + 2..];
        (after.starts_with(':')
            || after
                .strip_prefix(' ')
                .and_then(|suffix| suffix.get(..10))
                .is_some_and(valid_date))
        .then_some(index)
    })
}

/// トップレベルの `- **題（日付/印、補足）**: 本文` を読む。
pub fn parse(line: &str) -> Option<StampedTitle<'_>> {
    let rest = line.strip_prefix("- **")?;
    let close = title_closing_marker(rest)?;
    let bold = &rest[..close];
    let summary = rest[close + 2..].strip_prefix(':')?.trim();
    if summary.is_empty() || !bold.ends_with('）') {
        return None;
    }
    let open = bold.rfind('（')?;
    let title = &bold[..open];
    if title.trim().is_empty() {
        return None;
    }
    let inner = &bold[open + '（'.len_utf8()..bold.len() - '）'.len_utf8()];
    let date = inner.get(..10)?;
    if !valid_date(date) {
        return None;
    }
    let marked = inner.get(10..)?.strip_prefix('/')?;
    let (marker, supplement) = match marked.split_once('、') {
        Some((marker, note)) if !note.is_empty() => (marker, Some(note)),
        Some(_) => return None,
        None => (marked, None),
    };
    let (stamp, release_url) = if marker == "⏳" {
        (Stamp::Unreleased, None)
    } else if let Some(link) = marker.strip_prefix("[v") {
        let (raw, tail) = link.split_once("](")?;
        let raw = version(raw)?;
        let url = tail.strip_suffix(')')?;
        if !github_release_url(url, &format!("v{raw}")) {
            return None;
        }
        (Stamp::Released(raw), Some(url))
    } else {
        (Stamp::Released(version(marker.strip_prefix('v')?)?), None)
    };
    let metadata_start = "- **".len() + open;
    let stamp_start = metadata_start + '（'.len_utf8() + date.len();
    Some(StampedTitle {
        title,
        date,
        stamp,
        release_url,
        supplement,
        summary,
        metadata_range: metadata_start..metadata_start + bold.len() - open,
        stamp_range: stamp_start..stamp_start + 1 + marker.len(),
    })
}

/// 旧い括弧外の印は archive 等の読取互換だけに使う。
pub fn parse_legacy_compact(line: &str) -> Option<(&str, Stamp<'_>)> {
    let rest = line.strip_prefix("- **")?;
    let close = title_closing_marker(rest)?;
    let metadata = rest[close + 2..].strip_prefix(' ')?.split_once(':')?.0;
    let date = metadata.get(..10)?;
    if !valid_date(date) {
        return None;
    }
    let stamp = match metadata.get(10..)? {
        " ⏳" => Stamp::Unreleased,
        other => Stamp::Released(version(other.strip_prefix("/v")?)?),
    };
    Some((date, stamp))
}

/// YYYY-MM-DD の形の実在する日付か。
pub fn valid_date(date: &str) -> bool {
    if !(date.len() == 10
        && date.as_bytes().get(4) == Some(&b'-')
        && date.as_bytes().get(7) == Some(&b'-')
        && date
            .bytes()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit()))
    {
        return false;
    }
    let year = date[..4].parse::<u32>().expect("digits");
    let month = date[5..7].parse::<u32>().expect("digits");
    let day = date[8..10].parse::<u32>().expect("digits");
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => return false,
    };
    year > 0 && (1..=days).contains(&day)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_stamps_and_ranges() {
        let pending = "- **題（2026-11-05/⏳、補足）**: 要約";
        let item = parse(pending).unwrap();
        assert_eq!(item.title, "題");
        assert_eq!(item.date, "2026-11-05");
        assert_eq!(item.stamp, Stamp::Unreleased);
        assert_eq!(item.supplement, Some("補足"));
        assert_eq!(&pending[item.stamp_range], "/⏳");
        assert_eq!(&pending[item.metadata_range], "（2026-11-05/⏳、補足）");
        let released = "- **題（2026-11-05/[v1.2.3](https://github.com/owner/repo/releases/tag/v1.2.3)）**: 要約";
        let item = parse(released).unwrap();
        assert_eq!(item.stamp, Stamp::Released("1.2.3"));
        assert_eq!(
            item.release_url,
            Some("https://github.com/owner/repo/releases/tag/v1.2.3")
        );
        assert_eq!(
            parse("- **題（2026-11-05/v1.2.3）**: 要約").unwrap().stamp,
            Stamp::Released("1.2.3")
        );
    }
    #[test]
    fn rejects_invalid_or_legacy_stamps() {
        for item in [
            "- **題（2026-11-05）**: 要約",
            "- **題** 2026-11-05 ⏳: 要約",
            "- **題（2026-11-05/[v1.2.3](https://github.com/owner/repo/releases/tag/v1.2.4)）**: 要約",
            "- **題（2026-11-05/[v1.2.3](http://github.com/owner/repo/releases/tag/v1.2.3)）**: 要約",
            "- **題（2026-11-05/v1.02.3）**: 要約",
            "  - **題（2026-11-05/⏳）**: 要約",
        ] {
            assert!(parse(item).is_none(), "{item}");
        }
    }

    #[test]
    fn legacy_compact_does_not_read_bold_text_in_body() {
        let item = "- **成果（2026-11-05）**: 前版の **修正** 2026-11-04/v1.2.2: の続き";
        assert_eq!(parse_legacy_compact(item), None);
    }

    #[test]
    fn stamped_parser_does_not_read_bold_text_in_body() {
        let item = "- **題** 2026-11-05/v1.2.3: 本文 **強調（2026-11-05/v1.2.2）**: 続き";
        assert_eq!(parse(item), None);
    }

    #[test]
    fn nested_bold_in_title_keeps_stamp_and_legacy_date() {
        let stamped = "- **設定を **短縮** した（2026-06-13/⏳）**: 要約";
        let parsed = parse(stamped).unwrap();
        assert_eq!(parsed.title, "設定を **短縮** した");
        assert_eq!(parsed.date, "2026-06-13");
        assert_eq!(&stamped[parsed.metadata_range], "（2026-06-13/⏳）");
        let legacy = "- **設定を **短縮** した** 2026-06-13/v1.2.3: 要約";
        assert_eq!(
            parse_legacy_compact(legacy),
            Some(("2026-06-13", Stamp::Released("1.2.3")))
        );
    }
}
