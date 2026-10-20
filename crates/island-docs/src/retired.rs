//! 退役語（移設元: `check-retired-doc-identifiers.ts` と、`agent_docs.rs` の退役 skill 名の検査）。
//!
//! 形式と検査は島、語は各 repo の registry（[`RETIRED_REGISTRY_PATH`]）に置く（design §5.2）。
//! registry は `[{ "identifier", "retiredOn": "YYYY-MM-DD", "reason" }]`。語は完全な文字列一致で探し、
//! 表記揺れを推測しない。行に `<!-- retired-ok: 理由 -->` があればその行は許す。
//! 対象は repo 直下と `docs/` の Markdown（と scope 表の repo 行）。当時の記録（ADR・review・archive
//! など）は外す。

use serde_json::Value;

use crate::check::Input;
use crate::constants::{
    DOCS_DIR, MARKDOWN_EXTENSION, RETIRED_REGISTRY_PATH, RETIRED_SUPPRESSION_PREFIX,
    is_retired_extra_source, is_retired_historical,
};
use crate::rules::{Rule, RuleFinding};

/// registry の 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetiredTerm {
    pub identifier: String,
    pub retired_on: String,
    pub reason: String,
}

/// registry を読む。形式が違えば何件目が不正かを返す。
pub fn parse_registry(bytes: &[u8]) -> Result<Vec<RetiredTerm>, String> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| "registry が JSON ではありません".to_owned())?;
    let entries = value
        .as_array()
        .ok_or_else(|| "registry は配列にしてください".to_owned())?;
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let text = |key: &str| {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
            };
            match (text("identifier"), text("retiredOn"), text("reason")) {
                (Some(identifier), Some(retired_on), Some(reason)) if is_date(retired_on) => {
                    Ok(RetiredTerm {
                        identifier: identifier.to_owned(),
                        retired_on: retired_on.to_owned(),
                        reason: reason.to_owned(),
                    })
                }
                _ => Err(format!(
                    "registry の {} 件目が不正です（identifier / retiredOn: YYYY-MM-DD / reason）",
                    index + 1
                )),
            }
        })
        .collect()
}

fn is_date(value: &str) -> bool {
    value.len() == 10
        && value.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
}

fn in_scope(repo: Option<&str>, path: &str) -> bool {
    let docs_prefix = format!("{DOCS_DIR}/");
    path.ends_with(MARKDOWN_EXTENSION)
        && (!path.contains('/')
            || path.starts_with(&docs_prefix)
            || is_retired_extra_source(repo, path))
        && !is_retired_historical(repo, path)
}

fn is_suppressed(line: &str) -> bool {
    line.split_once(RETIRED_SUPPRESSION_PREFIX)
        .and_then(|(_, rest)| rest.split_once("-->"))
        .is_some_and(|(reason, _)| !reason.trim().is_empty())
}

pub fn check(input: &Input) -> Vec<RuleFinding> {
    let Some(bytes) = input.data.get(RETIRED_REGISTRY_PATH) else {
        return Vec::new();
    };
    let terms = match parse_registry(bytes) {
        Ok(terms) => terms,
        Err(message) => {
            return vec![RuleFinding::new(
                Rule::RetiredTerm,
                RETIRED_REGISTRY_PATH,
                message,
            )];
        }
    };
    let repo = input.repo.as_deref();
    let mut findings = Vec::new();
    for (path, text) in input.snapshot.markdown() {
        let (true, Ok(text)) = (in_scope(repo, path), text) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            if is_suppressed(line) {
                continue;
            }
            for term in terms.iter().filter(|term| line.contains(&term.identifier)) {
                findings.push(
                    RuleFinding::new(
                        Rule::RetiredTerm,
                        path,
                        format!(
                            "退役語 {} があります（{} 退役: {}）",
                            term.identifier, term.retired_on, term.reason
                        ),
                    )
                    .at(index + 1)
                    .related([RETIRED_REGISTRY_PATH]),
                );
            }
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGISTRY: &str =
        r#"[{"identifier": "/old-skill", "retiredOn": "2026-08-11", "reason": "改名した"}]"#;

    fn findings(repo: Option<&str>, registry: Option<&str>, files: &[(&str, &str)]) -> Vec<String> {
        let mut input = Input {
            repo: repo.map(str::to_owned),
            ..Input::default()
        };
        if let Some(registry) = registry {
            input.data.insert(
                RETIRED_REGISTRY_PATH.to_owned(),
                registry.as_bytes().to_vec(),
            );
        }
        for (path, text) in files {
            input.snapshot.add_markdown(path, text.as_bytes().to_vec());
        }
        check(&input).iter().map(ToString::to_string).collect()
    }

    #[test]
    fn registered_terms_fail_on_the_active_surface_only() {
        let files = [
            ("AGENTS.md", "x\n/old-skill を呼ぶ\n"),
            (
                "docs/guide.md",
                "/old-skill <!-- retired-ok: 移行の説明 -->\n/old-skill <!-- retired-ok: -->\n",
            ),
            ("docs/adr/0001-x.md", "/old-skill\n"),
            ("CHANGELOG.md", "/old-skill\n"),
            ("src/readme.md", "/old-skill\n"),
        ];
        assert_eq!(
            findings(None, Some(REGISTRY), &files),
            [
                "AGENTS.md:2: [retired-term] 退役語 /old-skill があります（2026-08-11 退役: 改名した）",
                "docs/guide.md:2: [retired-term] 退役語 /old-skill があります（2026-08-11 退役: 改名した）",
            ]
        );
        // registry の無い repo は検査しない。
        assert!(findings(None, None, &files).is_empty());
    }

    /// 範囲に足す行と履歴の行は scope 表の repo 行だけが持つ（fixture では example/meta）。行の無い
    /// repo の `.claude/skills/` は写しなので見ず、同じ path の履歴の記録は他 repo では見る。
    #[test]
    fn scope_table_rows_extend_and_exclude_only_for_their_repo() {
        let files = [
            (".claude/skills/a/SKILL.md", "/old-skill\n"),
            ("docs/old-improvements.md", "/old-skill\n"),
        ];
        assert_eq!(
            findings(Some("example/meta"), Some(REGISTRY), &files),
            [
                ".claude/skills/a/SKILL.md:1: [retired-term] 退役語 /old-skill があります（2026-08-11 退役: 改名した）"
            ]
        );
        assert_eq!(
            findings(Some("owner/other"), Some(REGISTRY), &files),
            [
                "docs/old-improvements.md:1: [retired-term] 退役語 /old-skill があります（2026-08-11 退役: 改名した）"
            ]
        );
    }

    #[test]
    fn a_malformed_registry_is_reported() {
        for registry in [
            "{}",
            "not json",
            r#"[{"identifier": "x", "retiredOn": "2026/08/11", "reason": "r"}]"#,
        ] {
            let found = findings(None, Some(registry), &[]);
            assert_eq!(found.len(), 1, "{registry}");
            assert!(found[0].starts_with(RETIRED_REGISTRY_PATH), "{found:?}");
        }
    }
}
