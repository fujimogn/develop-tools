use std::path::Path;

mod process {
    use std::{
        io,
        process::{Command, Output},
        time::Duration,
    };

    const MAX_SPAWN_ATTEMPTS: usize = 20;
    const SPAWN_RETRY_DELAY: Duration = Duration::from_millis(10);

    pub(crate) fn output_retrying_text_file_busy(command: &mut Command) -> io::Result<Output> {
        for attempt in 0..MAX_SPAWN_ATTEMPTS {
            match command.output() {
                Err(error)
                    if error.raw_os_error() == Some(libc::ETXTBSY)
                        && attempt + 1 < MAX_SPAWN_ATTEMPTS =>
                {
                    std::thread::sleep(SPAWN_RETRY_DELAY);
                }
                other => return other,
            }
        }
        unreachable!("final attempt always returns")
    }
}

pub mod dependency;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Light,
    Heavy,
}

// 記録文書だけを許す。ほかの docs と配布・CI 資材は heavy にする。
fn is_light_path(path: &str) -> bool {
    path == "TODO.md"
        || path == "CHANGELOG.md"
        || (path.starts_with("docs/reviews/")
            && path.ends_with(".md")
            && path.len() > "docs/reviews/.md".len())
}

pub fn classify_paths(paths: Option<&[String]>) -> Scope {
    match paths {
        Some(paths) if !paths.is_empty() && paths.iter().all(|path| is_light_path(path)) => {
            Scope::Light
        }
        _ => Scope::Heavy,
    }
}

fn valid_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn decide_push_scope(
    event: &str,
    forced: Option<bool>,
    before: &str,
    head: &str,
    paths: Option<&[String]>,
) -> Scope {
    if !matches!(event, "push" | "schedule" | "workflow_dispatch")
        || forced != Some(false)
        || !valid_sha(before)
        || !valid_sha(head)
        || before.bytes().all(|byte| byte == b'0')
    {
        return Scope::Heavy;
    }
    classify_paths(paths)
}

fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let mut command = develop_tools_core::git(root);
    command.args(args);
    let output = process::output_retrying_text_file_busy(&mut command).ok()?;
    output.status.success().then_some(output.stdout)
}

pub fn changed_paths(root: &Path, before: &str, head: &str) -> Option<Vec<String>> {
    if !valid_sha(before) || !valid_sha(head) {
        return None;
    }
    git(root, &["merge-base", "--is-ancestor", before, head])?;
    let range = format!("{before}..{head}");
    // -m は merge commit の差分も含め、--no-renames は旧・新 path を両方数える。
    let output = git(
        root,
        &[
            "log",
            "-m",
            "--format=",
            "--name-only",
            "-z",
            "--no-renames",
            &range,
            "--",
        ],
    )?;
    output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8(path.to_vec()).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Scope, classify_paths, decide_push_scope};

    fn paths(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn only_the_narrow_document_set_is_light() {
        assert_eq!(
            classify_paths(Some(&paths(&[
                "docs/reviews/a.md",
                "TODO.md",
                "CHANGELOG.md",
            ]))),
            Scope::Light
        );
        for outside in [
            "README.md",
            "docs/reviews/a.txt",
            "docs/reference/a.md",
            "openspec/x.md",
            ".github/workflows/ci.yml",
            "config/x.json",
        ] {
            assert_eq!(
                classify_paths(Some(&paths(&["docs/reviews/a.md", outside]))),
                Scope::Heavy
            );
        }
        assert_eq!(classify_paths(Some(&[])), Scope::Heavy);
        assert_eq!(classify_paths(None), Scope::Heavy);
    }

    #[test]
    fn unknown_or_non_push_input_is_heavy() {
        let sha = "a".repeat(40);
        let allowed = paths(&["TODO.md"]);
        assert_eq!(
            decide_push_scope("push", Some(false), &sha, &sha, Some(&allowed)),
            Scope::Light
        );
        for event in ["pull_request", "unknown"] {
            assert_eq!(
                decide_push_scope(event, Some(false), &sha, &sha, Some(&allowed)),
                Scope::Heavy
            );
        }
        for event in ["schedule", "workflow_dispatch"] {
            assert_eq!(
                decide_push_scope(event, Some(false), &sha, &sha, Some(&allowed)),
                Scope::Light
            );
        }
        assert_eq!(
            decide_push_scope("push", Some(true), &sha, &sha, Some(&allowed)),
            Scope::Heavy
        );
        assert_eq!(
            decide_push_scope("push", None, &sha, &sha, Some(&allowed)),
            Scope::Heavy
        );
        assert_eq!(
            decide_push_scope("push", Some(false), &"0".repeat(40), &sha, Some(&allowed)),
            Scope::Heavy
        );
        assert_eq!(
            decide_push_scope("push", Some(false), &sha, &sha, None),
            Scope::Heavy
        );
    }
}
