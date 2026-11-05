//! 刻印する repo の CHANGELOG の短い題。

use std::ops::Range;

pub const UNRELEASED_STAMP: &str = " ⏳";
pub const RELEASED_PREFIX: &str = "/v";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stamp<'a> {
    Unreleased,
    Released(&'a str),
}

#[derive(Debug, PartialEq, Eq)]
pub struct CompactTitle<'a> {
    pub title: &'a str,
    pub date: &'a str,
    pub stamp: Stamp<'a>,
    pub summary: &'a str,
    /// 日付の前の空白から印の末尾まで（AI 入力から取り除く範囲）。
    pub metadata_range: Range<usize>,
    /// 印だけの範囲（未リリースの印を版へ書き換える範囲）。
    pub stamp_range: Range<usize>,
}

/// トップレベルの短い題を読む。題は最初の閉じの `**` で終わる。
pub fn parse(line: &str) -> Option<CompactTitle<'_>> {
    let rest = line.strip_prefix("- **")?;
    let close = rest.find("**")?;
    let title = &rest[..close];
    if title.trim().is_empty() {
        return None;
    }
    let metadata_start = "- **".len() + close + 2;
    let metadata = line.get(metadata_start..)?.strip_prefix(' ')?;
    let date = metadata.get(..10)?;
    if !valid_date(date) {
        return None;
    }
    let (marker, summary) = metadata.get(10..)?.split_once(':')?;
    let summary = summary.trim();
    if summary.is_empty() {
        return None;
    }
    let stamp = if marker == UNRELEASED_STAMP {
        Stamp::Unreleased
    } else {
        let version = marker.strip_prefix(RELEASED_PREFIX)?;
        let mut parts = version.split('.');
        for _ in 0..3 {
            let part = parts.next()?;
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
        }
        if parts.next().is_some() {
            return None;
        }
        Stamp::Released(version)
    };
    let stamp_start = metadata_start + 1 + date.len();
    let end = stamp_start + marker.len();
    Some(CompactTitle {
        title,
        date,
        stamp,
        summary,
        metadata_range: metadata_start..end,
        stamp_range: stamp_start..end,
    })
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
    fn reads_compact_title_and_stamp_byte_ranges() {
        for (line, stamp) in [
            ("- **題** 2026-11-05 ⏳: 要約", Stamp::Unreleased),
            (
                "- **題** 2026-11-05/v1.20.3: 要約",
                Stamp::Released("1.20.3"),
            ),
        ] {
            let parsed = parse(line).unwrap();
            assert_eq!(parsed.title, "題");
            assert_eq!(parsed.date, "2026-11-05");
            assert_eq!(parsed.summary, "要約");
            assert_eq!(parsed.stamp, stamp);
            assert_eq!(
                &line[parsed.metadata_range],
                match stamp {
                    Stamp::Unreleased => " 2026-11-05 ⏳",
                    Stamp::Released(_) => " 2026-11-05/v1.20.3",
                }
            );
            assert_eq!(
                &line[parsed.stamp_range],
                match stamp {
                    Stamp::Unreleased => UNRELEASED_STAMP,
                    Stamp::Released(_) => "/v1.20.3",
                }
            );
        }
    }

    #[test]
    fn rejects_missing_or_malformed_compact_metadata() {
        for line in [
            "- **題** 2026-11-05: 要約",
            "- **題** 2026-11-05 ⏳️: 要約",
            "- **題**  2026-11-05 ⏳: 要約",
            "- **題** 2026-11-05  ⏳: 要約",
            "- **** 2026-11-05 ⏳: 要約",
            "- **  ** 2026-11-05 ⏳: 要約",
            "- **題** 2026-11-05 ⏳: ",
            "- **題** 2026-02-29 ⏳: 要約",
            "- **題** 0000-01-01 ⏳: 要約",
            "- **題** 2026-11-05/v1.2: 要約",
            "- **題** 2026-11-05/v1.2.3-rc1: 要約",
            "- **題** 2026-11-05/v1.2.3.4: 要約",
            "  - **題** 2026-11-05 ⏳: 要約",
            "- **題（2026-11-05）**: 要約",
        ] {
            assert!(parse(line).is_none(), "{line}");
        }
        assert!(parse("- **題** 2024-02-29/v0.0.1: 要約").is_some());
        assert!(parse("- **題** 2026-11-05/v１.2.3: 要約").is_none());
    }
}
