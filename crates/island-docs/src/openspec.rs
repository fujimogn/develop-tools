//! OpenSpec の active change の命名と必須 file（移設元: `documentation.test.ts` の検査）。
//!
//! strict validate（`openspec validate --all --strict`）は起動しない。各 repo の `spec:validate` が
//! 持つ（design §5.3。移設元の strict validate の起動はここへ移さない）。
//! 移設元との差: 名前の日付 prefix は有っても無くてもよい（design §5.2）。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::check::Input;
use crate::constants::{
    OPENSPEC_ARCHIVE_DIR_NAME, OPENSPEC_CHANGE_NAME_PATTERN, OPENSPEC_CHANGES_DIR,
    OPENSPEC_REQUIRED_FILES,
};
use crate::rules::{Rule, RuleFinding};

static CHANGE_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(OPENSPEC_CHANGE_NAME_PATTERN).expect("change name regex"));

pub fn check(input: &Input) -> Vec<RuleFinding> {
    let prefix = format!("{OPENSPEC_CHANGES_DIR}/");
    // change 名 → その dir 直下の file 名。
    let mut changes: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for path in input.snapshot.files() {
        let Some((name, rest)) = path
            .strip_prefix(&prefix)
            .and_then(|rest| rest.split_once('/'))
        else {
            continue;
        };
        if name == OPENSPEC_ARCHIVE_DIR_NAME {
            continue;
        }
        let direct = changes.entry(name).or_default();
        if !rest.contains('/') {
            direct.insert(rest);
        }
    }

    let mut findings = Vec::new();
    for (name, files) in changes {
        let dir = format!("{prefix}{name}");
        if !CHANGE_NAME.is_match(name) {
            findings.push(RuleFinding::new(
                Rule::OpenSpec,
                &dir,
                "change の名前は kebab-case（日付の prefix YYYY-MM-DD- は任意）にしてください",
            ));
        }
        for required in OPENSPEC_REQUIRED_FILES {
            if !files.contains(required) {
                findings.push(RuleFinding::new(
                    Rule::OpenSpec,
                    &dir,
                    format!("active change に {required} がありません"),
                ));
            }
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn findings(paths: &[&str]) -> Vec<String> {
        let mut input = Input::default();
        for path in paths {
            input.snapshot.add_path(path);
        }
        check(&input).iter().map(ToString::to_string).collect()
    }

    #[test]
    fn active_changes_need_a_kebab_name_and_the_required_files() {
        assert!(
            findings(&[
                "openspec/changes/add-x/proposal.md",
                "openspec/changes/add-x/tasks.md",
                "openspec/changes/add-x/specs/a/spec.md",
                "openspec/changes/2026-09-30-add-y/proposal.md",
                "openspec/changes/2026-09-30-add-y/tasks.md",
                "openspec/changes/.gitkeep",
                "openspec/changes/archive/2026-01-01-Old_Name/notes.md",
            ])
            .is_empty()
        );
        assert_eq!(
            findings(&[
                "openspec/changes/Add_X/proposal.md",
                "openspec/changes/Add_X/tasks.md",
                "openspec/changes/add-z/proposal.md",
                "openspec/changes/add-z/specs/tasks.md",
            ]),
            [
                "openspec/changes/Add_X: [openspec] change の名前は kebab-case（日付の prefix YYYY-MM-DD- は任意）にしてください",
                "openspec/changes/add-z: [openspec] active change に tasks.md がありません",
            ]
        );
    }
}
