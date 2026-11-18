//! Literal attribution editing on an input copy; Markdown spans are opaque.

use std::{collections::HashMap, ops::Range};

pub(super) fn strip(text: &str, actors: &[String]) -> String {
    if actors.is_empty() {
        return text.to_owned();
    }
    let Some(protected) = protected_bytes(text) else {
        return text.to_owned();
    };
    let mut removals = Vec::new();
    let mut stack = Vec::new();
    let mut start = 0;
    let mut separators = Vec::new();
    let mut valid = true;
    for (index, ch) in text.char_indices() {
        if protected[index] {
            continue;
        }
        match ch {
            '（' | '(' => {
                if stack.is_empty() {
                    start = index;
                    separators.clear();
                    valid = true;
                } else {
                    valid = false;
                }
                stack.push(ch);
            }
            '）' | ')' if !stack.is_empty() => {
                let opening = stack.pop().unwrap();
                valid &= matches!((opening, ch), ('（', '）') | ('(', ')'));
                if stack.is_empty() && valid {
                    attribution_ranges(
                        text,
                        start,
                        index,
                        ch.len_utf8(),
                        &separators,
                        actors,
                        &mut removals,
                    );
                }
            }
            '、' | ',' if stack.len() == 1 => separators.push((index, ch.len_utf8())),
            _ => {}
        }
    }
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    for range in removals {
        output.push_str(&text[cursor..range.start]);
        cursor = range.end;
    }
    output.push_str(&text[cursor..]);
    output
}

fn attribution_ranges(
    text: &str,
    open: usize,
    close: usize,
    close_len: usize,
    separators: &[(usize, usize)],
    actors: &[String],
    removals: &mut Vec<Range<usize>>,
) {
    let inside = open + text[open..].chars().next().unwrap().len_utf8();
    let mut boundaries = vec![inside];
    boundaries.extend(separators.iter().map(|(index, len)| index + len));
    let fields: Vec<_> = boundaries
        .iter()
        .enumerate()
        .map(|(i, start)| {
            let end = separators.get(i).map_or(close, |(index, _)| *index);
            &text[*start..end]
        })
        .collect();
    if fields.iter().any(|field| field.trim().is_empty()) {
        return;
    }
    let attributed: Vec<_> = fields
        .iter()
        .map(|field| {
            let field = field.trim();
            actors.iter().any(|actor| {
                field
                    .strip_prefix(actor.as_str())
                    .is_some_and(|tail| matches!(tail, "決定" | "依頼" | "の決定" | "の依頼"))
            })
        })
        .collect();
    if attributed.iter().all(|value| *value) {
        removals.push(open..close + close_len);
        return;
    }
    let mut i = 0;
    while i < attributed.len() {
        if !attributed[i] {
            i += 1;
            continue;
        }
        let first = i;
        while i < attributed.len() && attributed[i] {
            i += 1;
        }
        removals.push(if i == attributed.len() {
            separators[first - 1].0..close
        } else {
            boundaries[first]..boundaries[i]
        });
    }
}

fn run_len(bytes: &[u8], start: usize, marker: u8) -> usize {
    bytes[start..]
        .iter()
        .take_while(|byte| **byte == marker)
        .count()
}

fn escaped_end(text: &str, start: usize) -> usize {
    start + 1 + text[start + 1..].chars().next().map_or(0, char::len_utf8)
}

fn code_end(text: &str, start: usize, count: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = start + count;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let n = run_len(bytes, i, b'`');
            if n == count {
                return Some(i + n);
            }
            i += n;
        } else {
            i += 1;
        }
    }
    None
}

fn fence_end(text: &str, start: usize, count: usize) -> Option<usize> {
    let marker = text.as_bytes()[start];
    let first_line_end = text[start..].find('\n')? + start;
    let mut offset = first_line_end + 1;
    for line in text[offset..].split_inclusive('\n') {
        let trimmed = line.trim_start_matches(' ');
        let indentation = line.len() - trimmed.len();
        let n = if trimmed.as_bytes().first() == Some(&marker) {
            run_len(trimmed.as_bytes(), 0, marker)
        } else {
            0
        };
        if indentation <= 3 && n >= count && trimmed[n..].trim().is_empty() {
            return Some(offset + line.len());
        }
        offset += line.len();
    }
    None
}

fn fence_start(text: &str, start: usize) -> bool {
    let line = &text[text[..start].rfind('\n').map_or(0, |index| index + 1)..start];
    line.trim().is_empty() || line.trim() == "-"
}

// Cache nested pairs and unmatched opens: repeated ordinary '[' must not rescan the suffix.
fn bracket_end(
    text: &str,
    start: usize,
    pairs: &mut HashMap<usize, Option<usize>>,
) -> Option<usize> {
    if let Some(end) = pairs.get(&start) {
        return *end;
    }
    let bytes = text.as_bytes();
    let mut opens = vec![start];
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i = escaped_end(text, i),
            b'`' => match code_end(text, i, run_len(bytes, i, b'`')) {
                Some(end) => i = end,
                None => break,
            },
            b'[' => {
                opens.push(i);
                i += 1;
            }
            b']' => {
                let open = opens.pop().unwrap();
                i += 1;
                pairs.insert(open, Some(i));
                if opens.is_empty() {
                    return Some(i);
                }
            }
            _ => i += text[i..].chars().next()?.len_utf8(),
        }
    }
    for open in opens {
        pairs.insert(open, None);
    }
    None
}

// A non-link '[' is ordinary text. A recognized but unclosed destination is ambiguous.
fn link_end(
    text: &str,
    start: usize,
    pairs: &mut HashMap<usize, Option<usize>>,
) -> Result<Option<usize>, ()> {
    let Some(label_end) = bracket_end(text, start, pairs) else {
        return Ok(None);
    };
    let bytes = text.as_bytes();
    if bytes.get(label_end) == Some(&b'[') {
        return bracket_end(text, label_end, pairs).map(Some).ok_or(());
    }
    if bytes.get(label_end) != Some(&b'(') {
        return Ok(None);
    }
    let mut depth = 1;
    let mut i = label_end + 1;
    let mut quote = None;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            i = escaped_end(text, i);
            continue;
        }
        if let Some(q) = quote {
            if b == q {
                quote = None;
            }
        } else if (b == b'\'' || b == b'"')
            && i > label_end + 1
            && bytes[i - 1].is_ascii_whitespace()
        {
            quote = Some(b);
        } else if b == b'(' {
            depth += 1;
        } else if b == b')' {
            depth -= 1;
            if depth == 0 {
                return Ok(Some(i + 1));
            }
        }
        i += text[i..].chars().next().ok_or(())?.len_utf8();
    }
    Err(())
}

fn html_tag_end(text: &str, start: usize) -> Result<Option<usize>, ()> {
    let rest = &text[start..];
    for (opening, closing) in [("<!--", "-->"), ("<![CDATA[", "]]>"), ("<?", "?>")] {
        if rest.starts_with(opening) {
            return rest
                .find(closing)
                .map(|end| Some(start + end + closing.len()))
                .ok_or(());
        }
    }
    let declaration = rest.starts_with("<!");
    let name_start = start + 1 + usize::from(rest.starts_with("</") || declaration);
    let name_len = text[name_start..]
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'-')
        .count();
    if name_len == 0 || !text.as_bytes()[name_start].is_ascii_alphabetic() {
        return Ok(None);
    }
    let name_end = name_start + name_len;
    if !text
        .as_bytes()
        .get(name_end)
        .is_some_and(|b| b.is_ascii_whitespace() || matches!(*b, b'/' | b'>'))
    {
        return Ok(None);
    }
    let mut quote = None;
    for (relative, ch) in text[name_end..].char_indices() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch == '>' {
            let end = name_end + relative + 1;
            return Ok(Some(end));
        }
    }
    Err(())
}

// Treat a complete HTML element as opaque, including its text content.
fn html_end(text: &str, start: usize) -> Result<Option<usize>, ()> {
    let Some(end) = html_tag_end(text, start)? else {
        return Ok(None);
    };
    let rest = &text[start..end];
    if rest.starts_with("<!")
        || rest.starts_with("<?")
        || rest.starts_with("</")
        || rest.ends_with("/>")
    {
        return Ok(Some(end));
    }
    let name: String = rest[1..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let name = name.to_ascii_lowercase();
    if matches!(
        name.as_str(),
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    ) {
        return Ok(Some(end));
    }
    let mut depth = 1;
    let mut cursor = end;
    while let Some(relative) = text[cursor..].find('<') {
        let next = cursor + relative;
        let Some(next_end) = html_tag_end(text, next)? else {
            cursor = next + 1;
            continue;
        };
        let tag = &text[next..next_end];
        let closing = tag.starts_with("</");
        let offset = if closing { 2 } else { 1 };
        let other: String = tag[offset..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        if other.eq_ignore_ascii_case(&name) {
            if closing {
                depth -= 1;
            } else if !tag.ends_with("/>") {
                depth += 1;
            }
            if depth == 0 {
                return Ok(Some(next_end));
            }
        }
        cursor = next_end;
    }
    Err(())
}

fn protected_bytes(text: &str) -> Option<Vec<bool>> {
    let bytes = text.as_bytes();
    let mut protected = vec![false; bytes.len()];
    let mut pairs = HashMap::new();
    let mut i = 0;
    while i < bytes.len() {
        let end = match bytes[i] {
            b'\\' => Some(escaped_end(text, i)),
            b'`' => {
                let count = run_len(bytes, i, b'`');
                let inline = code_end(text, i, count);
                let before_newline = inline.is_some_and(|end| !text[i..end].contains('\n'));
                Some(if count >= 3 && fence_start(text, i) && !before_newline {
                    fence_end(text, i, count)?
                } else {
                    inline?
                })
            }
            b'~' if run_len(bytes, i, b'~') >= 3 && fence_start(text, i) => {
                Some(fence_end(text, i, run_len(bytes, i, b'~'))?)
            }
            b'[' => link_end(text, i, &mut pairs).ok()?,
            b'<' => html_end(text, i).ok()?,
            _ => None,
        };
        if let Some(end) = end {
            protected[i..end].fill(true);
            i = end;
        } else {
            i += text[i..].chars().next()?.len_utf8();
        }
    }
    Some(protected)
}

#[cfg(test)]
mod tests {
    use super::strip;

    #[test]
    fn literal_transform_is_idempotent_and_keeps_ambiguous_groups() {
        let actors = vec!["主体A".to_owned()];
        for (input, expected) in [
            ("成果（主体A依頼）", "成果"),
            ("成果（主体A決定、条件、主体A依頼）", "成果（条件）"),
            (
                "成果（x（主体A決定）、主体A依頼）",
                "成果（x（主体A決定）、主体A依頼）",
            ),
            ("成果（`a,b`、主体A決定）", "成果（`a,b`）"),
            ("成果（主体A依頼、、条件）", "成果（主体A依頼、、条件）"),
            (
                "[（主体A依頼）](./local)（主体A決定）",
                "[（主体A依頼）](./local)",
            ),
            (
                "<span>（主体A依頼）</span>（主体A決定）",
                "<span>（主体A依頼）</span>",
            ),
        ] {
            let result = strip(input, &actors);
            assert_eq!(result, expected);
            assert_eq!(strip(&result, &actors), result);
        }
        let prefix = format!("[ordinary {}", "[".repeat(4096));
        assert_eq!(strip(&format!("{prefix}（主体A依頼）"), &actors), prefix);
    }
}
