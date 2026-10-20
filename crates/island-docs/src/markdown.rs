//! Markdown の読み取り: fence 除外・inline code 除外・link 抽出・見出し anchor。
//!
//! link と anchor は**同じ fence 判定**（[`fence_mask`]）を使う。7 実装のうち、link と anchor で
//! fence の規則が割れていた実装（2 つあった）の事故を繰り返さない。
//!
//! 対象外（どの移設元も扱っていない）: setext 見出し（`===` 下線）、list / blockquote の中の fence、
//! 複数行にまたがる code span と link。

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

/// 行ごとに「fence の中（開始・終了の行を含む）なら true」を返す。
///
/// CommonMark 準拠: インデント 3 以下、``` か `~~~` を 3 文字以上。閉じは同じ文字で開きの長さ以上、
/// 後ろは空白だけ。backtick の fence は info string に backtick を含められない。閉じない fence は
/// 文書の終わりまで続く。
pub fn fence_mask(lines: &[&str]) -> Vec<bool> {
    let mut mask = Vec::with_capacity(lines.len());
    let mut open: Option<(char, usize)> = None;
    for line in lines {
        let fence = fence_marker(line);
        match (open, fence) {
            (None, Some((marker, length, rest))) => {
                if marker == '`' && rest.contains('`') {
                    mask.push(false);
                } else {
                    open = Some((marker, length));
                    mask.push(true);
                }
            }
            (Some((marker, length)), Some((close, close_length, rest)))
                if close == marker && close_length >= length && rest.trim().is_empty() =>
            {
                open = None;
                mask.push(true);
            }
            (Some(_), _) => mask.push(true),
            (None, None) => mask.push(false),
        }
    }
    mask
}

/// `^ {0,3}(`{3,}|~{3,})` に当たれば（文字、長さ、残り）を返す。
fn fence_marker(line: &str) -> Option<(char, usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let body = &line[indent..];
    let marker = body.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let length = body.len() - body.trim_start_matches(marker).len();
    (length >= 3).then(|| (marker, length, &body[length..]))
}

/// 行から code span を空白に置き換える（link 抽出の前に使う）。
///
/// CommonMark 準拠: 同じ長さの backtick の並びで閉じる。閉じが無い並びは文字として残す。
pub fn strip_code_spans(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'`' {
            let next = line[index..].find('`').map_or(line.len(), |at| index + at);
            out.push_str(&line[index..next]);
            index = next;
            continue;
        }
        let run = run_length(bytes, index);
        match find_closing_run(bytes, index + run, run) {
            Some(close) => {
                out.push_str(&" ".repeat(close + run - index));
                index = close + run;
            }
            None => {
                out.push_str(&line[index..index + run]);
                index += run;
            }
        }
    }
    out
}

fn run_length(bytes: &[u8], start: usize) -> usize {
    bytes[start..].iter().take_while(|b| **b == b'`').count()
}

fn find_closing_run(bytes: &[u8], mut from: usize, length: usize) -> Option<usize> {
    while from < bytes.len() {
        if bytes[from] == b'`' {
            let run = run_length(bytes, from);
            if run == length {
                return Some(from);
            }
            from += run;
        } else {
            from += 1;
        }
    }
    None
}

/// 文書中の link の宛先。`line` は 1 始まり。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkTarget {
    pub line: usize,
    pub target: String,
}

static REFERENCE_DEFINITION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^ {0,3}\[([^\]]+)\]:[ \t]*(?:<([^>]+)>|(\S+))").expect("reference regex")
});
static HTML_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)<a\b[^>]*?\s+href\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#)
        .expect("html link regex")
});

/// fence と code span を除いた本文から、inline link・image・reference 定義・HTML a href の宛先を順に返す。
///
/// 宛先は `<...>` 形式を外し、title（`"..."` / `'...'` / `(...)`）を含めない。footnote 定義
/// （`[^1]: 本文`）は link ではない。
pub fn link_targets(text: &str) -> Vec<LinkTarget> {
    let lines: Vec<&str> = text.lines().collect();
    let mask = fence_mask(&lines);
    let mut targets = Vec::new();
    for (index, (line, fenced)) in lines.iter().zip(mask).enumerate() {
        if fenced {
            continue;
        }
        let line_no = index + 1;
        let source = strip_code_spans(line);
        if let Some(captures) = REFERENCE_DEFINITION.captures(&source) {
            if !captures[1].starts_with('^') {
                let target = captures.get(2).or_else(|| captures.get(3));
                if let Some(target) = target {
                    targets.push(LinkTarget {
                        line: line_no,
                        target: unescape(target.as_str()),
                    });
                }
            }
            continue;
        }
        for target in inline_destinations(&source) {
            targets.push(LinkTarget {
                line: line_no,
                target: unescape(&target),
            });
        }
        for captures in HTML_LINK.captures_iter(&source) {
            if let Some(target) = captures
                .get(1)
                .or_else(|| captures.get(2))
                .or_else(|| captures.get(3))
            {
                targets.push(LinkTarget {
                    line: line_no,
                    target: target.as_str().to_owned(),
                });
            }
        }
    }
    targets
}

/// `](` ごとに宛先を読む。`[` が手前に無い `](` は link ではない。
///
/// `[` から対応を取らずに `](` を起点にするので、`[![img](a.png)](b.md)` の外側も拾える。
fn inline_destinations(line: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut search = 0;
    while let Some(found) = line[search..].find("](") {
        let close = search + found;
        let start = close + 2;
        search = start;
        if !line[..close].contains('[') {
            continue;
        }
        if let Some((target, end)) = parse_destination(&line[start..]) {
            targets.push(target);
            search = start + end;
        }
    }
    targets
}

/// `(` の直後から宛先と title を読み、（宛先、`)` の次の位置）を返す。形が崩れていれば None。
fn parse_destination(rest: &str) -> Option<(String, usize)> {
    let skipped = rest.len() - rest.trim_start().len();
    let body = &rest[skipped..];
    let (target, used) = if let Some(inner) = body.strip_prefix('<') {
        let end = inner.find(['>', '\n'])?;
        if !inner[end..].starts_with('>') {
            return None;
        }
        (inner[..end].to_owned(), end + 2)
    } else {
        let mut depth = 0usize;
        let mut end = body.len();
        let mut escaped = false;
        for (at, c) in body.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            match c {
                '\\' => escaped = true,
                '(' => depth += 1,
                ')' if depth == 0 => {
                    end = at;
                    break;
                }
                ')' => depth -= 1,
                c if c.is_whitespace() => {
                    end = at;
                    break;
                }
                _ => {}
            }
        }
        (body[..end].to_owned(), end)
    };
    let after = &body[used..];
    let closing = title_end(after)?;
    if target.is_empty() {
        return None;
    }
    Some((target, skipped + used + closing))
}

/// CommonMark の backslash escape（`\` + ASCII 記号）を外す。terraform-docs は `#input\_name` と書く。
fn unescape(target: &str) -> String {
    let mut out = String::with_capacity(target.len());
    let mut chars = target.chars().peekable();
    while let Some(c) = chars.next() {
        match chars.peek() {
            Some(next) if c == '\\' && next.is_ascii_punctuation() => {
                out.push(*next);
                chars.next();
            }
            _ => out.push(c),
        }
    }
    out
}

/// 宛先の後ろ（空白・任意の title・空白）を読み、`)` の次の位置を返す。
fn title_end(after: &str) -> Option<usize> {
    let skipped = after.len() - after.trim_start().len();
    let body = &after[skipped..];
    if body.starts_with(')') {
        return Some(skipped + 1);
    }
    if skipped == 0 {
        return None;
    }
    let close = match body.chars().next()? {
        '"' => '"',
        '\'' => '\'',
        '(' => ')',
        _ => return None,
    };
    let end = body[1..].find(close)? + 1;
    let tail = &body[end + 1..];
    let tail_skip = tail.len() - tail.trim_start().len();
    tail[tail_skip..]
        .starts_with(')')
        .then_some(skipped + end + 1 + tail_skip + 1)
}

static HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^ {0,3}#{1,6}[ \t]+(.+?)[ \t]*#*[ \t]*$").expect("heading regex")
});
static HTML_ANCHOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)<a\s+(?:id|name)=["']([^"']+)["'][^>]*>"#).expect("html anchor regex")
});
static HTML_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").expect("tag regex"));
static CODE_SPAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`([^`]*)`").expect("code span regex"));
static LINK_TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\([^)]*\)").expect("link text regex"));
static NOT_SLUG_CHAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}\p{M}\s_-]").expect("slug char regex"));

/// GitHub の見出し slug。
///
/// 規則は移設元のうち修正済みの 2 実装に揃える（design §5.1）:
/// - HTML タグ・code span の backtick・link 記法を剥がし、小文字にする
/// - 異体字セレクタ（U+FE0E / U+FE0F）と ZWJ を先に落とす。`\p{M}` に含まれるので、残すと
///   `## ⚠️ 見出し` が `️-見出し` になる（未修正の移設元があった）
/// - 文字・数字・結合文字・空白・`_`・`-` 以外を落とす（`_` と `\p{M}` は残す）
/// - **trim せず、空白を 1 文字ずつ `-` にする**（`## ⚠️ A — B` は `-a--b`）
pub fn github_slug(heading: &str) -> String {
    let text = HTML_TAG.replace_all(heading, "");
    let text = CODE_SPAN.replace_all(&text, "$1");
    let text = LINK_TEXT.replace_all(&text, "$1");
    let lower: String = text
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '\u{FE0E}' | '\u{FE0F}' | '\u{200D}'))
        .collect();
    NOT_SLUG_CHAR
        .replace_all(&lower, "")
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .collect()
}

/// ATX 見出しの行なら、その本文（`#` と前後の空白、閉じの `#` を除いたもの）を返す。fence の中かどうかは
/// 呼び出し側が [`fence_mask`] で見る。
pub fn heading_text(line: &str) -> Option<&str> {
    HEADING
        .captures(line)
        .and_then(|captures| captures.get(1))
        .map(|text| text.as_str())
}

/// 文書が持つ anchor の集合。ATX 見出しの slug（重複は `-1`、`-2`）と `<a id>` / `<a name>`。
pub fn heading_anchors(text: &str) -> HashSet<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mask = fence_mask(&lines);
    let mut anchors = HashSet::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (line, fenced) in lines.iter().zip(mask) {
        if fenced {
            continue;
        }
        if let Some(captures) = HEADING.captures(line) {
            let base = github_slug(&captures[1]);
            if !base.is_empty() {
                let count = counts.entry(base.clone()).or_insert(0);
                let slug = if *count == 0 {
                    base.clone()
                } else {
                    format!("{base}-{count}")
                };
                *count += 1;
                anchors.insert(slug);
            }
        }
        for captures in HTML_ANCHOR.captures_iter(line) {
            anchors.insert(captures[1].to_owned());
        }
    }
    anchors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(text: &str) -> Vec<String> {
        link_targets(text).into_iter().map(|t| t.target).collect()
    }

    #[test]
    fn fence_closes_only_with_same_marker_and_enough_length() {
        let lines = ["````", "```", "````", "after"];
        assert_eq!(fence_mask(&lines), [true, true, true, false]);
        let lines = ["~~~", "```", "~~~", "after"];
        assert_eq!(fence_mask(&lines), [true, true, true, false]);
    }

    #[test]
    fn closing_fence_must_not_have_trailing_text() {
        let lines = ["```", "``` not a close", "```", "after"];
        assert_eq!(fence_mask(&lines), [true, true, true, false]);
    }

    #[test]
    fn four_space_indent_is_not_a_fence_and_backtick_info_cannot_contain_backticks() {
        assert_eq!(fence_mask(&["    ```", "x"]), [false, false]);
        assert_eq!(fence_mask(&["``` a`b", "x"]), [false, false]);
    }

    #[test]
    fn unclosed_fence_runs_to_the_end() {
        assert_eq!(fence_mask(&["```", "[x](a.md)"]), [true, true]);
    }

    #[test]
    fn code_spans_match_equal_length_runs() {
        assert_eq!(
            strip_code_spans("a `[x](y)` b"),
            format!("a {} b", " ".repeat(8))
        );
        assert_eq!(
            strip_code_spans("``a ` b`` c"),
            format!("{} c", " ".repeat(9))
        );
        assert_eq!(strip_code_spans("lone ` tick"), "lone ` tick");
    }

    #[test]
    fn destinations_drop_titles_and_angle_brackets() {
        assert_eq!(
            targets("[a](a.md \"t\") [b](<b c.md>) [c](c.md 't') [d](d.md (t))"),
            ["a.md", "b c.md", "c.md", "d.md"]
        );
    }

    #[test]
    fn nested_image_link_yields_both() {
        assert_eq!(targets("[![i](i.png)](b.md)"), ["i.png", "b.md"]);
    }

    #[test]
    fn html_link_reads_href_not_data_href() {
        assert_eq!(
            targets("<a data-href='https://example.com/' href='/home/u/out.md'>x</a>"),
            ["/home/u/out.md"]
        );
    }

    #[test]
    fn backslash_escapes_are_removed_from_destinations() {
        assert_eq!(
            targets(r"[a](#input\_zone\_id) [b](a\).md) [c](a\b.md)"),
            ["#input_zone_id", "a).md", r"a\b.md"]
        );
    }

    #[test]
    fn balanced_parentheses_stay_in_the_destination() {
        assert_eq!(targets("[a](a(1).md)"), ["a(1).md"]);
    }

    #[test]
    fn reference_definitions_but_not_footnotes() {
        assert_eq!(
            targets("[ref]: ./r.md \"t\"\n[^1]: 本文です\n[x]: <s p.md>"),
            ["./r.md", "s p.md"]
        );
    }

    #[test]
    fn prose_brackets_without_opener_are_not_links() {
        assert!(targets("foo](bar.md)").is_empty());
        assert!(targets("[a]() と [b](x.md").is_empty());
    }
}
