//! AI の Release 本文の照合（design §5 の 3・4）。
//!
//! 核（`island-llm`）は出力が 1 個の JSON 値であることまでを見る。ここでは中身の形・禁止の語と形・
//! 入力の番号との対応を見て、通らなければ `output_rejected:<位置>.<種別>`（共通のエラー形式）を返す。
//! 入力の項目が出力に現れなくても拒まず、本文の末尾で compare へ案内する（D4）。

use std::collections::BTreeSet;

use develop_tools_core::bounded_name::contains_bounded_name;
use island_llm::settings::{Audience, DictionaryMissing, Settings};
use serde_json::{Map, Value};

use crate::release_notes::ReleaseExtract;

/// 辞書か内部名の一覧が環境側に無い。public は推論せず fallback、private は推論して理由に出す。
pub const DICTIONARY_MISSING: &str = "dictionary_missing";

/// 長い数字の並びとみなす桁数（電話番号・口座番号・個人番号の形）。
const LONG_DIGITS: usize = 10;

/// URL の形（小文字で照合）。
const URL_MARKERS: &[&str] = &["http:", "https:", "www.", "://"];

/// 鍵・token の頭（小文字で照合）。
const TOKEN_PREFIXES: &[&str] = &[
    "sk-ant-",
    "github_pat_",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
];

/// 後ろに `:` か `=` が続けば秘密の値の形とみなす語（小文字で照合）。
const SECRET_LABELS: &[&str] = &["password", "secret"];

/// 照合の材料。辞書の語はどちらの audience でも、内部名は public だけで照合する。
#[derive(Debug, Default)]
pub struct Checks {
    forbidden_terms: Vec<String>,
    internal_names: Vec<String>,
    /// private で辞書が無いまま推論する。本文の理由の行に [`DICTIONARY_MISSING`] を出す。
    pub dictionary_missing: bool,
}

/// 推論の前に照合の材料を決める。public で辞書か内部名が無ければ推論しない（理由 code を返す）。
/// private は内部名を使わないので、禁止語だけを読む。
pub fn prepare(settings: &Settings, audience: Audience) -> Result<Checks, &'static str> {
    match audience {
        Audience::Public => {
            let dictionary = settings
                .dictionary()
                .map_err(|DictionaryMissing| DICTIONARY_MISSING)?;
            Ok(Checks {
                forbidden_terms: dictionary.forbidden_terms,
                internal_names: dictionary.internal_names,
                dictionary_missing: false,
            })
        }
        Audience::Private => Ok(match settings.forbidden_terms() {
            Some(terms) => Checks {
                forbidden_terms: terms.to_vec(),
                ..Checks::default()
            },
            None => Checks {
                dictionary_missing: true,
                ..Checks::default()
            },
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiItem {
    pub text: String,
    /// 入力の 1 始まりの番号（`numbered_ai_input` の番号）。
    pub sources: Vec<usize>,
}

/// 照合を通った AI の本文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiNotes {
    pub summary: String,
    pub items: Vec<AiItem>,
    /// どの出力の項目も参照しなかった入力の番号。
    pub unreferenced: Vec<usize>,
}

fn rejection(location: &str, kind: &str) -> String {
    format!("output_rejected:{location}.{kind}")
}

/// 鍵の一覧どおりの object か。足りない鍵は `<鍵>.required`、余分な鍵は `unrecognized_keys`。
fn object<'a>(
    value: &'a Value,
    location: &str,
    keys: &[&str],
) -> Result<&'a Map<String, Value>, String> {
    let map = value
        .as_object()
        .ok_or_else(|| rejection(location, "invalid_type"))?;
    if map.keys().any(|key| !keys.contains(&key.as_str())) {
        return Err(rejection(location, "unrecognized_keys"));
    }
    Ok(map)
}

fn field<'a>(map: &'a Map<String, Value>, key: &str, location: &str) -> Result<&'a Value, String> {
    map.get(key).ok_or_else(|| rejection(location, "required"))
}

fn has_long_digits(text: &str) -> bool {
    let mut run = 0;
    text.chars().any(|c| {
        run = if c.is_ascii_digit() || ('０'..='９').contains(&c) {
            run + 1
        } else {
            0
        };
        run >= LONG_DIGITS
    })
}

/// `<` の直後が英字・`/`・`!`・`?` なら tag の形（`->` や `a < b` は通す）。
fn has_html_tag(text: &str) -> bool {
    text.match_indices('<').any(|(start, _)| {
        text[start + 1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'))
    })
}

fn has_secret_label(lower: &str) -> bool {
    SECRET_LABELS.iter().any(|label| {
        lower.match_indices(label).any(|(start, _)| {
            lower[start + label.len()..]
                .trim_start()
                .starts_with([':', '='])
        })
    })
}

/// 文の照合。最初に掛かった種別を返す。
fn text_kind(text: &str, checks: &Checks) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    if text.contains(['\n', '\r']) {
        Some("format")
    } else if URL_MARKERS.iter().any(|marker| lower.contains(marker)) {
        Some("url")
    } else if has_html_tag(text) {
        Some("html")
    } else if TOKEN_PREFIXES.iter().any(|prefix| lower.contains(prefix)) || has_secret_label(&lower)
    {
        Some("token")
    } else if has_long_digits(text) {
        Some("digits")
    } else if checks
        .forbidden_terms
        .iter()
        .filter(|term| !term.is_empty())
        .any(|term| lower.contains(&term.to_ascii_lowercase()))
    {
        Some("term")
    } else if checks
        .internal_names
        .iter()
        .any(|name| contains_bounded_name(text, name, true))
    {
        Some("internal_name")
    } else {
        None
    }
}

fn checked_text(value: &Value, location: &str, checks: &Checks) -> Result<String, String> {
    let text = value
        .as_str()
        .ok_or_else(|| rejection(location, "invalid_type"))?
        .trim();
    if text.is_empty() {
        return Err(rejection(location, "too_small"));
    }
    match text_kind(text, checks) {
        Some(kind) => Err(rejection(location, kind)),
        None => Ok(text.to_owned()),
    }
}

fn checked_sources(
    value: &Value,
    location: &str,
    input_count: usize,
) -> Result<Vec<usize>, String> {
    let values = value
        .as_array()
        .ok_or_else(|| rejection(location, "invalid_type"))?;
    if values.is_empty() {
        return Err(rejection(location, "too_small"));
    }
    let mut sources = Vec::with_capacity(values.len());
    for value in values {
        let number = value
            .as_u64()
            .ok_or_else(|| rejection(location, "invalid_type"))?;
        let number = usize::try_from(number)
            .ok()
            .filter(|number| (1..=input_count).contains(number))
            .ok_or_else(|| rejection(location, "out_of_range"))?;
        sources.push(number);
    }
    Ok(sources)
}

/// 核が返した JSON 値を照合する。`input_count` は AI へ送った入力の項目数。
pub fn check_output(value: &Value, input_count: usize, checks: &Checks) -> Result<AiNotes, String> {
    let root = object(value, "root", &["summary", "items"])?;
    let summary = checked_text(field(root, "summary", "summary")?, "summary", checks)?;
    let values = field(root, "items", "items")?
        .as_array()
        .ok_or_else(|| rejection("items", "invalid_type"))?;
    if values.is_empty() {
        return Err(rejection("items", "too_small"));
    }
    let mut items = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let location = format!("items.{index}");
        let item = object(value, &location, &["text", "sources"])?;
        let text_location = format!("{location}.text");
        let sources_location = format!("{location}.sources");
        items.push(AiItem {
            text: checked_text(field(item, "text", &text_location)?, &text_location, checks)?,
            sources: checked_sources(
                field(item, "sources", &sources_location)?,
                &sources_location,
                input_count,
            )?,
        });
    }
    let referenced: BTreeSet<_> = items.iter().flat_map(|item| item.sources.iter()).collect();
    let unreferenced = (1..=input_count)
        .filter(|number| !referenced.contains(number))
        .collect();
    Ok(AiNotes {
        summary,
        items,
        unreferenced,
    })
}

impl AiNotes {
    /// 要約・項目・compare の順の本文。現れなかった入力があれば compare へ案内する（D4）。
    pub fn render(&self, extract: &ReleaseExtract) -> String {
        let items: String = self
            .items
            .iter()
            .map(|item| format!("- {}\n", item.text))
            .collect();
        let tail = if self.unreferenced.is_empty() {
            extract.compare_line()
        } else {
            format!(
                "その他の変更は {} を参照してください。",
                extract.compare_link()
            )
        };
        format!("{}\n\n{items}\n{tail}\n", self.summary)
    }
}
