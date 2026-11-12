//! Release repo の CHANGELOG の括弧内刻印。
use crate::develop_tools_manifest::NotesLanguage;
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
    parse_with_language(line, NotesLanguage::Ja)
}

struct TitleSyntax {
    opening: &'static str,
    closing: &'static str,
    supplement: &'static str,
    summary: &'static str,
    // 英語の開き文字列に含む、括弧の前の区切り空白。
    leading_space_bytes: usize,
}

const JAPANESE_SYNTAX: TitleSyntax = TitleSyntax {
    opening: "（",
    closing: "）",
    supplement: "、",
    summary: ":",
    leading_space_bytes: 0,
};
const ENGLISH_SYNTAX: TitleSyntax = TitleSyntax {
    opening: " (",
    closing: ")",
    supplement: ", ",
    summary: ": ",
    leading_space_bytes: 1,
};

fn title_syntax(language: NotesLanguage) -> &'static TitleSyntax {
    match language {
        NotesLanguage::Ja => &JAPANESE_SYNTAX,
        NotesLanguage::En => &ENGLISH_SYNTAX,
    }
}

struct TitleMetadata<'a> {
    title: &'a str,
    date: &'a str,
    suffix: &'a str,
    summary: &'a str,
    range: Range<usize>,
    suffix_start: usize,
}

// 日付型と刻印型の外形を同じ文法で読み、印の有無だけを各入口で判定する。
fn title_metadata(line: &str, language: NotesLanguage) -> Option<TitleMetadata<'_>> {
    let syntax = title_syntax(language);
    let rest = line.strip_prefix("- **")?;
    let close = title_closing_marker(rest)?;
    let bold = &rest[..close];
    let summary = rest[close + 2..].strip_prefix(syntax.summary)?.trim();
    if summary.is_empty() || !bold.ends_with(syntax.closing) {
        return None;
    }
    let open = match language {
        NotesLanguage::Ja => bold.rfind(syntax.opening),
        NotesLanguage::En => bold.rmatch_indices(syntax.opening).find_map(|(index, _)| {
            bold.get(index + syntax.opening.len()..index + syntax.opening.len() + 10)
                .is_some_and(valid_date)
                .then_some(index)
        }),
    }?;
    let title = &bold[..open];
    if title.trim().is_empty() || (language == NotesLanguage::En && title != title.trim_end()) {
        return None;
    }
    let inner = &bold[open + syntax.opening.len()..bold.len() - syntax.closing.len()];
    let date = inner.get(..10)?;
    if !valid_date(date) {
        return None;
    }
    Some(TitleMetadata {
        title,
        date,
        suffix: &inner[10..],
        summary,
        range: "- **".len() + open + syntax.leading_space_bytes.."- **".len() + bold.len(),
        suffix_start: "- **".len() + open + syntax.opening.len() + date.len(),
    })
}

/// 印の無い日付型の成果項目を検査し、完了日を返す。
pub fn parse_dated(line: &str, language: NotesLanguage) -> Option<&str> {
    let metadata = title_metadata(line, language)?;
    if !metadata.suffix.is_empty() {
        let supplement = metadata
            .suffix
            .strip_prefix(title_syntax(language).supplement)?;
        if supplement.trim().is_empty() {
            return None;
        }
    }
    Some(metadata.date)
}

/// 宣言された言語の括弧・区切りで刻印を読む。既存の入口は日本語を保つ。
pub fn parse_with_language(line: &str, language: NotesLanguage) -> Option<StampedTitle<'_>> {
    let metadata = title_metadata(line, language)?;
    let marked = metadata.suffix.strip_prefix('/')?;
    let (marker, supplement) = match marked.split_once(title_syntax(language).supplement) {
        Some((marker, note))
            if !note.is_empty() && (language == NotesLanguage::Ja || !note.trim().is_empty()) =>
        {
            (marker, Some(note))
        }
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
    Some(StampedTitle {
        title: metadata.title,
        date: metadata.date,
        stamp,
        release_url,
        supplement,
        summary: metadata.summary,
        metadata_range: metadata.range,
        stamp_range: metadata.suffix_start..metadata.suffix_start + 1 + marker.len(),
    })
}

/// 表示用に日付と印を一度だけ除き、題・補足・本文の byte を保つ。
/// 戻り値を再度この関数へ渡さず、抽出時に一度だけ呼ぶ。
pub fn render_without_stamp(line: &str, language: NotesLanguage) -> Option<String> {
    let parsed = parse_with_language(line, language)?;
    let syntax = title_syntax(language);
    let replacement = parsed.supplement.map_or_else(String::new, |note| {
        format!("{}{note}{}", syntax.opening, syntax.closing)
    });
    let range = parsed.metadata_range.start - syntax.leading_space_bytes..parsed.metadata_range.end;
    let mut rendered = line.to_owned();
    rendered.replace_range(range, &replacement);
    Some(rendered)
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

/// 月内の版小節。言語によらず ASCII の版と ISO 日付を使う。
pub fn parse_version_heading(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("### ")?.trim_end_matches('\r');
    let (release, date) = rest.strip_suffix(')')?.split_once(" (")?;
    Some((version(release)?, valid_date(date).then_some(date)?))
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
    use crate::develop_tools_manifest::NotesLanguage;

    #[test]
    fn english_stamps_preserve_metadata_and_byte_ranges() {
        let line = "- **Tests (2026-11-09/⏳, fast (pane))**: Keeps ordering deterministic.";
        let item = parse_with_language(line, NotesLanguage::En).unwrap();
        assert_eq!(item.title, "Tests");
        assert_eq!(item.date, "2026-11-09");
        assert_eq!(item.stamp, Stamp::Unreleased);
        assert_eq!(item.supplement, Some("fast (pane)"));
        assert_eq!(&line[item.stamp_range], "/⏳");
        assert_eq!(&line[item.metadata_range], "(2026-11-09/⏳, fast (pane))");
        assert!(parse(line).is_none());
    }

    #[test]
    fn english_stamp_grammar_rejects_mixed_or_empty_metadata() {
        for line in [
            "- **Stop  (2026-11-09/⏳)**: Body",
            "- **Stop (2026-11-09/⏳)**:Body",
            "- **Stop（2026-11-09/⏳）**: Body",
            "- **Stop (2026-11-09/⏳、note)**: Body",
            "- **Stop (2026-11-09/⏳,  )**: Body",
            "- **Stop (2026-11-09/⏳/v1.2.3)**: Body",
            "- **Stop (2026-11-09/[v1.2.3](https://github.com/owner/repo/releases/tag/v1.2.4))**: Body",
            "- **Stop (2026-11-09/[v1.2.3](/releases/tag/v1.2.3))**: Body",
        ] {
            assert!(
                parse_with_language(line, NotesLanguage::En).is_none(),
                "{line}"
            );
        }
        for marker in [
            "v1.2.3",
            "[v1.2.3](https://github.com/owner/repo/releases/tag/v1.2.3)",
        ] {
            let line = format!("- **History (2026-11-09/{marker})**: Preserves the note.");
            assert_eq!(
                parse_with_language(&line, NotesLanguage::En).unwrap().stamp,
                Stamp::Released("1.2.3")
            );
        }
    }
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
