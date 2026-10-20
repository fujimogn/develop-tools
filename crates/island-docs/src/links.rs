//! local link と anchor の解決（design §5.1）。git に触れず、[`Snapshot`] だけを見る。
//!
//! 抽出・解決は移設元の `markdown-local-links.test.ts` の規則: percent-decode、`?query` 除去、
//! 元文書の dir からの相対解決、repo 外の検出、tracked の file と dir で存在確認、`.md` の fragment は
//! 見出し anchor と照合。移設元は `/` で始まる path を黙って飛ばしていたが、島では拒否する。
//! 手元の絶対 path（[`HOST_PATH_PREFIXES`]）は通常拒否し、島の scope 表で許可した repo だけ通す。行番号の fragment
//! （[`LINE_ANCHOR_PATTERN`]）は見出しと照合しない。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::LazyLock;

use regex::Regex;

use crate::constants::{
    EXTERNAL_SCHEME_PATTERN, HOST_PATH_PREFIXES, LINE_ANCHOR_PATTERN, MARKDOWN_EXTENSION,
};
use crate::markdown::{heading_anchors, link_targets};

/// tracked の path 集合と、読む必要のある Markdown の本文。path は repo root からの `/` 区切り。
#[derive(Debug, Default)]
pub struct Snapshot {
    files: BTreeSet<String>,
    dirs: BTreeSet<String>,
    markdown: BTreeMap<String, Result<String, NotUtf8>>,
}

/// UTF-8 として読めない本文。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotUtf8;

impl Snapshot {
    /// tracked の path を 1 つ足す。親 dir も存在する path として数える。
    pub fn add_path(&mut self, path: &str) {
        let mut dir = path;
        while let Some((parent, _)) = dir.rsplit_once('/') {
            self.dirs.insert(parent.to_owned());
            dir = parent;
        }
        self.files.insert(path.to_owned());
    }

    /// tracked の Markdown の本文を足す（path も足す）。
    pub fn add_markdown(&mut self, path: &str, bytes: Vec<u8>) {
        self.add_path(path);
        let text = String::from_utf8(bytes).map_err(|_| NotUtf8);
        self.markdown.insert(path.to_owned(), text);
    }

    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(String::as_str)
    }

    pub fn markdown(&self) -> impl Iterator<Item = (&str, Result<&str, NotUtf8>)> {
        self.markdown
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_deref().map_err(|e| *e)))
    }

    fn exists(&self, path: &str) -> bool {
        path.is_empty() || self.files.contains(path) || self.dirs.contains(path)
    }
}

/// link が切れている理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// 宛先の path が tracked に無い。
    MissingPath,
    /// 宛先の Markdown にその見出しが無い。
    MissingAnchor,
    /// `../` で repo の外へ出る。
    OutsideRepository,
    /// `/` で始まる path。手元の FS の絶対 path として解決されてしまう。
    AbsolutePath,
    /// 端末固有の path または `file://` URL。
    HostPath,
    /// percent-encoding を解けない、または UTF-8 にならない。
    InvalidEncoding,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MissingPath => "リンク先が tracked にありません",
            Self::MissingAnchor => "リンク先に見出しがありません",
            Self::OutsideRepository => "repo の外を指しています",
            Self::AbsolutePath => "/ で始まる path は使えません（相対 path にしてください）",
            Self::HostPath => "端末の path を指しています",
            Self::InvalidEncoding => "percent-encoding を解けません",
        })
    }
}

/// 切れた link 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokenLink {
    pub source: String,
    pub line: usize,
    pub target: String,
    /// 解決できたときの宛先 path（repo root からの相対）。staged の絞り込みに使う。
    pub resolved: Option<String>,
    pub problem: Problem,
}

/// 宛先の分類。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// 外部（scheme 付き、`//host`）。検査しない。
    External,
    /// 島の scope 表で明示して許可した端末 path。
    AllowedHostPath,
    /// repo 内の path と、あれば decode 済みの fragment。
    Local {
        path: String,
        fragment: Option<String>,
    },
    Rejected(Problem),
}

static EXTERNAL_SCHEME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(EXTERNAL_SCHEME_PATTERN).expect("scheme regex"));
static LINE_ANCHOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(LINE_ANCHOR_PATTERN).expect("line anchor regex"));

/// `source`（repo 内の Markdown）に書かれた `target` を解決する。
pub fn resolve(source: &str, target: &str) -> Resolution {
    resolve_with_policy(source, target, false)
}

fn resolve_with_policy(source: &str, target: &str, allow_host_paths: bool) -> Resolution {
    if target.starts_with("file://") {
        return Resolution::Rejected(Problem::HostPath);
    }
    if HOST_PATH_PREFIXES
        .iter()
        .any(|prefix| target.starts_with(prefix))
    {
        return if allow_host_paths {
            Resolution::AllowedHostPath
        } else {
            Resolution::Rejected(Problem::HostPath)
        };
    }
    if target.starts_with("//") || EXTERNAL_SCHEME.is_match(target) {
        return Resolution::External;
    }
    if target.starts_with('/') {
        return Resolution::Rejected(Problem::AbsolutePath);
    }
    let (encoded_path, encoded_fragment) = match target.split_once('#') {
        Some((path, fragment)) => (path, Some(fragment)),
        None => (target, None),
    };
    let encoded_path = encoded_path.split('?').next().unwrap_or_default();
    let (Some(path), Some(fragment)) = (
        percent_decode(encoded_path),
        encoded_fragment.map_or(Some(None), |f| percent_decode(f).map(Some)),
    ) else {
        return Resolution::Rejected(Problem::InvalidEncoding);
    };
    let fragment = fragment.filter(|f| !f.is_empty());
    if path.is_empty() {
        return Resolution::Local {
            path: source.to_owned(),
            fragment,
        };
    }
    let base = source.rsplit_once('/').map_or("", |(dir, _)| dir);
    match normalize(base, &path) {
        Some(path) => Resolution::Local { path, fragment },
        None => Resolution::Rejected(Problem::OutsideRepository),
    }
}

/// `base` dir から `relative` を辿る。repo root より上へ出たら None。
fn normalize(base: &str, relative: &str) -> Option<String> {
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    Some(parts.join("/"))
}

/// `decodeURIComponent` と同じ判定: `%` の後に 16 進 2 桁が無い、または UTF-8 にならなければ None。
fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = input.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `sources` の各 Markdown の link を検査する。
pub fn check<'a>(
    snapshot: &Snapshot,
    sources: impl IntoIterator<Item = &'a str>,
    allow_host_paths: bool,
) -> Vec<BrokenLink> {
    let mut broken = Vec::new();
    let mut anchor_cache: BTreeMap<&str, Option<std::collections::HashSet<String>>> =
        BTreeMap::new();
    for source in sources {
        let Some(Ok(text)) = snapshot.markdown.get(source) else {
            continue;
        };
        for link in link_targets(text) {
            let push = |broken: &mut Vec<BrokenLink>, resolved: Option<String>, problem| {
                broken.push(BrokenLink {
                    source: source.to_owned(),
                    line: link.line,
                    target: link.target.clone(),
                    resolved,
                    problem,
                });
            };
            match resolve_with_policy(source, &link.target, allow_host_paths) {
                Resolution::External | Resolution::AllowedHostPath => {}
                Resolution::Rejected(problem) => push(&mut broken, None, problem),
                Resolution::Local { path, fragment } => {
                    if !snapshot.exists(&path) {
                        push(&mut broken, Some(path), Problem::MissingPath);
                        continue;
                    }
                    let Some(fragment) = fragment else { continue };
                    if !path.ends_with(MARKDOWN_EXTENSION) || LINE_ANCHOR.is_match(&fragment) {
                        continue;
                    }
                    let key = snapshot
                        .markdown
                        .get_key_value(&path)
                        .map(|(k, _)| k.as_str());
                    let Some(key) = key else { continue };
                    let anchors = anchor_cache.entry(key).or_insert_with(|| {
                        snapshot.markdown[key].as_deref().ok().map(heading_anchors)
                    });
                    // UTF-8 で読めない宛先は anchor を判定できない（元文書側で報告する）。
                    if anchors.as_ref().is_some_and(|set| !set.contains(&fragment)) {
                        push(&mut broken, Some(path), Problem::MissingAnchor);
                    }
                }
            }
        }
    }
    broken
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(path: &str, fragment: Option<&str>) -> Resolution {
        Resolution::Local {
            path: path.to_owned(),
            fragment: fragment.map(str::to_owned),
        }
    }

    #[test]
    fn resolves_relative_to_the_source_directory() {
        assert_eq!(resolve("docs/a.md", "./b.md"), local("docs/b.md", None));
        assert_eq!(
            resolve("docs/a.md", "../README.md"),
            local("README.md", None)
        );
        assert_eq!(resolve("docs/a.md", "sub/"), local("docs/sub", None));
    }

    #[test]
    fn fragment_only_points_at_the_source() {
        assert_eq!(resolve("docs/a.md", "#x"), local("docs/a.md", Some("x")));
    }

    #[test]
    fn query_is_dropped_and_percent_encoding_is_decoded() {
        assert_eq!(
            resolve("a.md", "b%20c.md?plain=1#%E8%A6%8B"),
            local("b c.md", Some("見"))
        );
        assert_eq!(
            resolve("a.md", "b%zz.md"),
            Resolution::Rejected(Problem::InvalidEncoding)
        );
        assert_eq!(
            resolve("a.md", "b%ff.md"),
            Resolution::Rejected(Problem::InvalidEncoding)
        );
    }

    #[test]
    fn outside_and_absolute_are_rejected() {
        assert_eq!(
            resolve("docs/a.md", "../../x.md"),
            Resolution::Rejected(Problem::OutsideRepository)
        );
        assert_eq!(
            resolve("a.md", "/etc/hosts"),
            Resolution::Rejected(Problem::AbsolutePath)
        );
    }

    #[test]
    fn host_paths_are_rejected() {
        for target in [
            "/home/u/.config/x.md",
            "/Users/u/Develop/x.md#h",
            "~/work/x.md",
            "file:///home/u/x.md",
        ] {
            assert_eq!(
                resolve("a.md", target),
                Resolution::Rejected(Problem::HostPath),
                "{target}"
            );
        }
        for target in ["/etc/hosts", "/homework.md", "/Users", "~user/x.md"] {
            assert_ne!(
                resolve("a.md", target),
                Resolution::Rejected(Problem::HostPath),
                "{target}"
            );
        }
    }

    fn broken(files: &[(&str, &str)]) -> Vec<(String, Problem)> {
        let mut snapshot = Snapshot::default();
        for (path, text) in files {
            snapshot.add_markdown(path, text.as_bytes().to_vec());
        }
        let sources: Vec<&str> = files.iter().map(|(path, _)| *path).collect();
        check(&snapshot, sources, false)
            .into_iter()
            .map(|link| (link.target, link.problem))
            .collect()
    }

    #[test]
    fn line_anchors_pass_without_a_heading() {
        let text = "[a](b.md#L40) [r](b.md#L10-L20) [s](#L3)\n";
        assert_eq!(broken(&[("a.md", text), ("b.md", "# b\n")]), []);
    }

    /// 行番号の形を外れたものは見出しとして照合する（無ければ切れ）。
    #[test]
    fn near_line_anchors_are_still_headings() {
        let text = "[a](b.md#L) [b](b.md#l40) [c](b.md#L10-20) [d](b.md#L4x) [e](b.md#xL40)\n";
        let found = broken(&[("a.md", text), ("b.md", "# b\n")]);
        assert_eq!(found.len(), 5, "{found:?}");
        assert!(
            found
                .iter()
                .all(|(_, problem)| *problem == Problem::MissingAnchor)
        );
    }

    /// 行番号の fragment でも path の存在は確かめる。
    #[test]
    fn line_anchor_to_a_missing_file_is_broken() {
        assert_eq!(
            broken(&[("a.md", "[a](gone.md#L4)\n")]),
            [("gone.md#L4".to_owned(), Problem::MissingPath)]
        );
    }

    #[test]
    fn schemes_and_protocol_relative_are_external() {
        for target in ["https://x", "mailto:a@b", "app://x", "HTTP://X", "//cdn/x"] {
            assert_eq!(resolve("a.md", target), Resolution::External, "{target}");
        }
        assert_eq!(
            resolve("a.md", "http-notes.md"),
            local("http-notes.md", None)
        );
    }
}
