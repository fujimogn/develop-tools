//! Hash-guarded byte regions shared by focused distribution adapters.
use sha2::{Digest, Sha256};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkerFormat {
    id: &'static str,
    line_prefix: &'static [u8],
    accepted: &'static [&'static str],
}

impl MarkerFormat {
    pub const fn new(id: &'static str, line_prefix: &'static [u8]) -> Self {
        Self {
            id,
            line_prefix,
            accepted: &[],
        }
    }

    /// 書くのは常に今の名前空間。`namespaces` の区画も読み、`plan` で今の名前空間へ書き換える。
    /// 受け入れていない名前空間の区画は、ふつうの本文として扱う。
    pub const fn accepting(self, namespaces: &'static [&'static str]) -> Self {
        Self {
            accepted: namespaces,
            ..self
        }
    }

    pub const fn id(self) -> &'static str {
        self.id
    }
    pub const fn line_prefix(self) -> &'static [u8] {
        self.line_prefix
    }
}

const NEW_NAMESPACE: &str = "develop-tools";

/// 読む名前空間と、その BEGIN・END の token。先頭が今の名前空間。
fn marker_tokens(format: MarkerFormat) -> Vec<(&'static str, Vec<u8>, Vec<u8>)> {
    std::iter::once(NEW_NAMESPACE)
        .chain(format.accepted.iter().copied())
        .map(|namespace| {
            (
                namespace,
                format!("<!-- BEGIN {namespace}:").into_bytes(),
                format!("<!-- END {namespace}:").into_bytes(),
            )
        })
        .collect()
}

/// repo の root `AGENTS.md` に配る規約の区画。配布と docs 検査が同じ形式を引く。
pub const AGENTS_MD: MarkerFormat = MarkerFormat::new("AGENTS.md", b"");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockState {
    Current,
    Stale,
    Edited,
    Missing,
}

pub struct LegacyRegion<'a> {
    pub expected: &'a [u8],
    pub range: Range<usize>,
}

pub struct BlockPlan {
    state: BlockState,
    source_sha256: String,
    target_sha256: String,
    updated_target: Vec<u8>,
}

impl BlockPlan {
    pub fn state(&self) -> BlockState {
        self.state
    }

    pub fn source_sha256(&self) -> &str {
        &self.source_sha256
    }

    pub fn target_sha256(&self) -> &str {
        &self.target_sha256
    }

    pub fn updated_target(&self) -> &[u8] {
        &self.updated_target
    }
}

struct ExistingBlock<'a> {
    namespace: &'static str,
    range: Range<usize>,
    body_range: Range<usize>,
    body: &'a [u8],
    hash_matches: bool,
}

fn lines(bytes: &[u8]) -> impl Iterator<Item = (usize, &[u8])> {
    let mut offset = 0;
    bytes
        .split_inclusive(|byte| *byte == b'\n')
        .map(move |line| {
            let start = offset;
            offset += line.len();
            (start, line)
        })
}

fn trim_line_ending(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hash(bytes)
}

fn find_all(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    haystack
        .windows(needle.len())
        .enumerate()
        .filter_map(|(offset, bytes)| (bytes == needle).then_some(offset))
        .collect()
}

fn identifier_end(bytes: &[u8]) -> usize {
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b' ' || bytes[offset..].starts_with(b"-->") {
            return offset;
        }
        offset += 1;
    }
    bytes.len()
}

fn marker_lines<'a>(
    content: &'a [u8],
    format: MarkerFormat,
) -> Result<Option<ExistingBlock<'a>>, &'static str> {
    if !valid_format(format) {
        return Err("managed_block_marker_invalid");
    }

    let tokens = marker_tokens(format);
    let mut begins: Vec<(usize, &'a [u8], usize, &'static str)> = Vec::new();
    let mut ends: Vec<(usize, &'a [u8], usize, &'static str)> = Vec::new();

    for (offset, raw_line) in lines(content) {
        let line = trim_line_ending(raw_line);
        for (namespace, begin_token, end_token) in &tokens {
            let namespace = *namespace;
            for position in find_all(line, begin_token) {
                let after_token = position + begin_token.len();
                let tail = &line[after_token..];
                let id_end = identifier_end(tail);
                if tail[..id_end] == *format.id.as_bytes() {
                    if position != format.line_prefix.len()
                        || !line.starts_with(format.line_prefix)
                        || !raw_line.ends_with(b"\n")
                    {
                        return Err("managed_block_marker_invalid");
                    }
                    begins.push((offset, raw_line, position, namespace));
                }
            }
            for position in find_all(line, end_token) {
                let after_token = position + end_token.len();
                let tail = &line[after_token..];
                let id_end = identifier_end(tail);
                if tail[..id_end] == *format.id.as_bytes() {
                    if position != format.line_prefix.len()
                        || !line.starts_with(format.line_prefix)
                        || !raw_line.ends_with(b"\n")
                    {
                        return Err("managed_block_marker_invalid");
                    }
                    ends.push((offset, raw_line, position, namespace));
                }
            }
        }
    }

    if begins.is_empty() && ends.is_empty() {
        return Ok(None);
    }
    if begins.len() != 1 || ends.len() != 1 {
        return Err("managed_block_marker_invalid");
    }

    let (start, begin_raw, begin_position, namespace) = begins[0];
    let (end, end_raw, end_position, end_namespace) = ends[0];
    if start >= end
        || namespace != end_namespace
        || begin_position != format.line_prefix.len()
        || end_position != format.line_prefix.len()
    {
        return Err("managed_block_marker_invalid");
    }

    let begin_line = trim_line_ending(begin_raw);
    let end_line = trim_line_ending(end_raw);
    let begin_line = &begin_line[format.line_prefix.len()..];
    let end_line = &end_line[format.line_prefix.len()..];

    let begin_prefix = format!("<!-- BEGIN {namespace}:{} v1 sha256=", format.id);
    let Some(declared) = begin_line.strip_prefix(begin_prefix.as_bytes()) else {
        return Err("managed_block_marker_invalid");
    };
    let Some(declared) = declared.strip_suffix(b" -->") else {
        return Err("managed_block_marker_invalid");
    };
    if declared.len() != 64
        || !declared
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err("managed_block_marker_invalid");
    }

    let expected_end = format!("<!-- END {namespace}:{} -->", format.id);
    if end_line != expected_end.as_bytes() {
        return Err("managed_block_marker_invalid");
    }

    let body_start = start + begin_raw.len();
    if body_start >= end || content[end - 1] != b'\n' {
        return Err("managed_block_marker_invalid");
    }
    let body = &content[body_start..end];
    let range = start..end + end_raw.len();
    Ok(Some(ExistingBlock {
        namespace,
        range,
        body_range: body_start..end,
        body,
        hash_matches: hash(body).as_bytes() == declared,
    }))
}

fn valid_format(format: MarkerFormat) -> bool {
    !format.id.is_empty()
        && format
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
        && format
            .line_prefix
            .iter()
            .all(|byte| matches!(byte, b' ' | b'\t' | b'#'))
        && !format.line_prefix.contains(&b'\n')
        && !format.line_prefix.contains(&b'\r')
}

fn validate_source(format: MarkerFormat, source: &[u8]) -> Result<(), &'static str> {
    if source.is_empty()
        || !source.ends_with(b"\n")
        || marker_tokens(format).iter().any(|(_, begin, end)| {
            [begin, end].iter().any(|marker| {
                source
                    .windows(marker.len())
                    .any(|part| part == marker.as_slice())
            })
        })
    {
        return Err("managed_block_source_invalid");
    }
    Ok(())
}

pub fn managed_range(
    format: MarkerFormat,
    target: &[u8],
) -> Result<Option<Range<usize>>, &'static str> {
    Ok(marker_lines(target, format)?.map(|block| block.range))
}

/// 本文が marker の hash と一致する区画の範囲。marker の不正と手編集は `None`。
pub fn intact_range(format: MarkerFormat, target: &[u8]) -> Option<Range<usize>> {
    let block = marker_lines(target, format).ok()??;
    block.hash_matches.then_some(block.range)
}

pub fn managed_body_range(
    format: MarkerFormat,
    target: &[u8],
) -> Result<Option<Range<usize>>, &'static str> {
    Ok(marker_lines(target, format)?.map(|block| block.body_range))
}

/// 区画が受け入れた旧い名前空間で書かれているか。
pub fn uses_legacy_namespace(format: MarkerFormat, target: &[u8]) -> Result<bool, &'static str> {
    Ok(marker_lines(target, format)?.is_some_and(|block| block.namespace != NEW_NAMESPACE))
}

pub fn inspect(
    format: MarkerFormat,
    source: &[u8],
    target: &[u8],
) -> Result<BlockState, &'static str> {
    validate_source(format, source)?;
    match marker_lines(target, format)? {
        None => Ok(BlockState::Missing),
        Some(block) if !block.hash_matches => Ok(BlockState::Edited),
        Some(block) if block.body == source => Ok(BlockState::Current),
        Some(_) => Ok(BlockState::Stale),
    }
}

fn render_block(format: MarkerFormat, source: &[u8]) -> Vec<u8> {
    let mut rendered = Vec::new();
    rendered.extend_from_slice(format.line_prefix);
    rendered.extend_from_slice(
        format!(
            "<!-- BEGIN {NEW_NAMESPACE}:{} v1 sha256={} -->\n",
            format.id,
            hash(source)
        )
        .as_bytes(),
    );
    rendered.extend_from_slice(source);
    rendered.extend_from_slice(format.line_prefix);
    rendered.extend_from_slice(format!("<!-- END {NEW_NAMESPACE}:{} -->\n", format.id).as_bytes());
    rendered
}

fn splice(target: &[u8], range: Range<usize>, replacement: &[u8]) -> Vec<u8> {
    let mut updated =
        Vec::with_capacity(target.len() - (range.end - range.start) + replacement.len());
    updated.extend_from_slice(&target[..range.start]);
    updated.extend_from_slice(replacement);
    updated.extend_from_slice(&target[range.end..]);
    updated
}

fn validate_legacy(target: &[u8], legacy: &LegacyRegion<'_>) -> Result<(), &'static str> {
    let range = &legacy.range;
    if range.start >= range.end
        || range.end > target.len()
        || (range.start != 0 && target[range.start - 1] != b'\n')
        || target.get(range.end.saturating_sub(1)) != Some(&b'\n')
        || target[range.clone()] != *legacy.expected
    {
        return Err("managed_block_legacy_mismatch");
    }
    Ok(())
}

pub fn plan(
    format: MarkerFormat,
    source: &[u8],
    target: &[u8],
    legacy: Option<LegacyRegion<'_>>,
) -> Result<BlockPlan, &'static str> {
    validate_source(format, source)?;
    let existing = marker_lines(target, format)?;
    let state = match (&existing, &legacy) {
        (Some(_), Some(_)) => return Err("managed_block_legacy_ambiguous"),
        (Some(block), None) if !block.hash_matches => return Err("managed_block_edited"),
        (Some(block), None) if block.body == source && block.namespace == NEW_NAMESPACE => {
            BlockState::Current
        }
        (Some(_), None) => BlockState::Stale,
        (None, Some(region)) => {
            validate_legacy(target, region)?;
            BlockState::Stale
        }
        (None, None) => BlockState::Missing,
    };

    let rendered = render_block(format, source);
    let updated_target = match (&existing, &legacy, state) {
        (Some(_), None, BlockState::Current) => target.to_vec(),
        (Some(block), None, BlockState::Stale) => splice(target, block.range.clone(), &rendered),
        (None, Some(region), BlockState::Stale) => splice(target, region.range.clone(), &rendered),
        (None, None, BlockState::Missing) => {
            let mut appended = target.to_vec();
            if !appended.is_empty() {
                appended.push(b'\n');
            }
            appended.extend_from_slice(&rendered);
            appended
        }
        _ => return Err("managed_block_state_invalid"),
    };

    Ok(BlockPlan {
        state,
        source_sha256: hash(source),
        target_sha256: hash(target),
        updated_target,
    })
}

/// marker の hash と本文が合わない（手で直された）区画を、今の `source` で置き換える plan。
/// 利用者が上書きを明示したときだけ使う。手で直していない区画と区画の無い target は `plan` に任せる。
pub fn plan_overwrite(
    format: MarkerFormat,
    source: &[u8],
    target: &[u8],
) -> Result<BlockPlan, &'static str> {
    validate_source(format, source)?;
    let block = marker_lines(target, format)?
        .filter(|block| !block.hash_matches)
        .ok_or("managed_block_not_edited")?;
    Ok(BlockPlan {
        state: BlockState::Edited,
        source_sha256: hash(source),
        target_sha256: hash(target),
        updated_target: splice(target, block.range, &render_block(format, source)),
    })
}

/// marker も既存 block も無い target の `offset`（行頭）へ管理 block を挿入する plan。
/// 置換元が無い新規 repo の採用に使う。marker が既にあれば通常の `plan` を使う。
pub fn plan_insert(
    format: MarkerFormat,
    source: &[u8],
    target: &[u8],
    offset: usize,
) -> Result<BlockPlan, &'static str> {
    validate_source(format, source)?;
    if marker_lines(target, format)?.is_some() {
        return Err("managed_block_present");
    }
    if offset > target.len() || (offset != 0 && target[offset - 1] != b'\n') {
        return Err("managed_block_insert_offset_invalid");
    }
    let rendered = render_block(format, source);
    Ok(BlockPlan {
        state: BlockState::Missing,
        source_sha256: hash(source),
        target_sha256: hash(target),
        updated_target: splice(target, offset..offset, &rendered),
    })
}

pub fn apply(
    plan: &BlockPlan,
    current_source: &[u8],
    current_target: &[u8],
) -> Result<Vec<u8>, &'static str> {
    if hash(current_source) != plan.source_sha256 {
        return Err("managed_block_source_changed");
    }
    if hash(current_target) != plan.target_sha256 {
        return Err("managed_block_target_changed");
    }
    Ok(plan.updated_target.clone())
}
