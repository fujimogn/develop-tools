//! Conservative proof for the narrow record-only path set. Unknown commands and
//! source syntax are errors, so adding a new check cannot silently retain light.
use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use serde_json::Value;

#[derive(Clone, Copy)]
enum ConfigPattern {
    Name(&'static str),
    NamePrefix(&'static str),
    NamePrefixSuffix(&'static str, &'static str),
    PathComponent(&'static str),
    CargoConfig,
}

impl ConfigPattern {
    fn matches(self, path: &str) -> bool {
        let name = path.rsplit('/').next().unwrap_or(path);
        match self {
            Self::Name(expected) => name == expected,
            Self::NamePrefix(prefix) => name.starts_with(prefix),
            Self::NamePrefixSuffix(prefix, suffix) => {
                name.starts_with(prefix) && name.ends_with(suffix)
            }
            Self::PathComponent(component) => path.split('/').any(|part| part == component),
            Self::CargoConfig => {
                let mut parts = path.rsplit('/');
                parts.next().is_some_and(|name| name.starts_with("config"))
                    && parts.next() == Some(".cargo")
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ConfigProof {
    Unsupported,
    PackageScripts,
}

// Runtime inputs are listed once. Package scripts (including pre/post lifecycle
// hooks) are traversed below; other formats need an adapter parser in 2.1.
const EXECUTION_CONFIGS: &[(ConfigPattern, ConfigProof)] = &[
    (ConfigPattern::NamePrefix(".env"), ConfigProof::Unsupported),
    (ConfigPattern::Name("bunfig.toml"), ConfigProof::Unsupported),
    (ConfigPattern::Name(".npmrc"), ConfigProof::Unsupported),
    (ConfigPattern::CargoConfig, ConfigProof::Unsupported),
    (
        ConfigPattern::NamePrefix("rust-toolchain"),
        ConfigProof::Unsupported,
    ),
    (
        ConfigPattern::NamePrefixSuffix("tsconfig", ".json"),
        ConfigProof::Unsupported,
    ),
    (
        ConfigPattern::PathComponent("node_modules"),
        ConfigProof::Unsupported,
    ),
    (
        ConfigPattern::Name("package.json"),
        ConfigProof::PackageScripts,
    ),
];

fn tracked(root: &Path) -> Result<BTreeSet<String>, String> {
    let mut command = develop_tools_core::git(root);
    command.args(["ls-files", "-z", "--cached"]);
    let output = crate::process::output_retrying_text_file_busy(&mut command)
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err("git ls-files failed".into());
    }
    output
        .stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8(s.to_vec()).map_err(|_| "non-UTF8 tracked path".into()))
        .collect()
}

struct Tree<'a> {
    root: &'a Path,
    files: BTreeSet<String>,
}

fn string_map(value: &yaml_serde::Value) -> bool {
    value.as_mapping().is_some_and(|map| {
        map.iter()
            .all(|(key, value)| key.as_str().is_some() && value.as_str().is_some())
    })
}

fn concurrency(value: &yaml_serde::Value) -> bool {
    value.as_mapping().is_some_and(|map| {
        map.iter().all(|(key, value)| match key.as_str() {
            Some("group") => value.as_str().is_some(),
            Some("cancel-in-progress") => value.as_bool().is_some() || value.as_str().is_some(),
            _ => false,
        })
    })
}

fn workflow_field(key: &str, value: &yaml_serde::Value) -> bool {
    match key {
        "name" | "run-name" => value.as_str().is_some(),
        "on" => {
            value.as_mapping().is_some()
                || value.as_sequence().is_some()
                || value.as_str().is_some()
        }
        "permissions" => string_map(value),
        "concurrency" => concurrency(value),
        "jobs" => value.as_mapping().is_some(),
        _ => false,
    }
}

fn job_field(key: &str, value: &yaml_serde::Value) -> bool {
    match key {
        "name" | "if" => value.as_str().is_some(),
        "runs-on" => value.as_str() == Some("ubuntu-latest"),
        "needs" => {
            value.as_str().is_some()
                || value
                    .as_sequence()
                    .is_some_and(|items| items.iter().all(|item| item.as_str().is_some()))
        }
        "steps" => value.as_sequence().is_some(),
        "outputs" | "permissions" => string_map(value),
        "concurrency" => concurrency(value),
        "timeout-minutes" => value.as_u64().is_some(),
        _ => false,
    }
}

fn step_field(key: &str, value: &yaml_serde::Value) -> bool {
    match key {
        "name" | "id" | "if" | "run" | "uses" | "working-directory" => value.as_str().is_some(),
        "continue-on-error" => value.as_bool() == Some(false),
        _ => false,
    }
}

impl Tree<'_> {
    fn read(&self, path: &str) -> Result<String, String> {
        let relative = Path::new(path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            || !self.files.contains(path)
        {
            return Err(format!("untracked or unsafe path: {path}"));
        }
        let full = self.root.join(relative);
        if full
            .symlink_metadata()
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err(format!("symlink: {path}"));
        }
        let bytes = fs::read(&full).map_err(|e| e.to_string())?;
        let mut command = develop_tools_core::git(self.root);
        command.args(["show", &format!(":{path}")]);
        let index = crate::process::output_retrying_text_file_busy(&mut command)
            .map_err(|e| e.to_string())?;
        if !index.status.success() || index.stdout != bytes {
            return Err(format!("index mismatch: {path}"));
        }
        String::from_utf8(bytes).map_err(|_| format!("non-UTF8: {path}"))
    }
}

fn workflow_entries(tree: &Tree<'_>) -> Result<Vec<(String, String, String)>, String> {
    let workflows: Vec<_> = tree
        .files
        .iter()
        .filter(|p| {
            p.starts_with(".github/workflows/") && (p.ends_with(".yml") || p.ends_with(".yaml"))
        })
        .collect();
    if workflows.is_empty() {
        return Err("no workflows".into());
    }
    let mut entries = Vec::new();
    let mut standard_seen = false;
    for path in workflows {
        let value: yaml_serde::Value = yaml_serde::from_str(&tree.read(path)?)
            .map_err(|e| format!("invalid workflow {path}: {e}"))?;
        let fields = value.as_mapping().ok_or("non-mapping workflow")?;
        if fields
            .iter()
            .any(|(key, value)| !key.as_str().is_some_and(|key| workflow_field(key, value)))
        {
            return Err(format!("unresolved workflow setting: {path}"));
        }
        let jobs = value
            .get("jobs")
            .and_then(yaml_serde::Value::as_mapping)
            .ok_or_else(|| format!("no jobs: {path}"))?;
        if jobs.is_empty() {
            return Err(format!("empty jobs: {path}"));
        }
        if path.as_str() == ".github/workflows/ci-develop.yml" {
            standard_seen = true;
            let common_job = jobs.get("common").ok_or("missing common job")?;
            if common_job.get("if").is_some()
                || common_job.get("continue-on-error").is_some()
                || common_job.get("defaults").is_some()
                || common_job.get("env").is_some()
            {
                return Err("conditional or ignored common job".into());
            }
            let common = common_job
                .get("steps")
                .and_then(yaml_serde::Value::as_sequence)
                .ok_or("missing common job")?;
            if !common.iter().any(|step| {
                step.get("run")
                    .and_then(yaml_serde::Value::as_str)
                    .is_some_and(|run| {
                        matches!(
                            run.trim(),
                            "develop-docs-check --tracked"
                                | "tools/target/debug/develop-docs-check --tracked"
                        )
                    })
                    && step.get("continue-on-error").is_none()
                    && step.get("working-directory").is_none()
                    && step.get("env").is_none()
                    && step.get("shell").is_none()
                    && step.get("if").is_none_or(|condition| {
                        condition.as_str() == Some("github.event_name == 'push'")
                    })
            }) {
                return Err("light documentation check missing".into());
            }
            if jobs.get("scope").is_none() {
                return Err("scope job missing".into());
            }
        }
        for (name, job) in jobs {
            let name = name.as_str().ok_or("non-string job")?;
            let fields = job.as_mapping().ok_or("non-mapping job")?;
            if fields
                .iter()
                .any(|(key, value)| !key.as_str().is_some_and(|key| job_field(key, value)))
            {
                return Err(format!("unresolved job setting: {path}:{name}"));
            }
            if path.as_str() == ".github/workflows/ci-develop.yml"
                && matches!(name, "common" | "scope")
            {
                continue;
            }
            let steps = job
                .get("steps")
                .and_then(yaml_serde::Value::as_sequence)
                .ok_or_else(|| format!("unknown job: {path}:{name}"))?;
            for (index, step) in steps.iter().enumerate() {
                let fields = step.as_mapping().ok_or("non-mapping step")?;
                if fields
                    .iter()
                    .any(|(key, value)| !key.as_str().is_some_and(|key| step_field(key, value)))
                {
                    return Err(format!("unresolved step setting: {path}:{name}:{index}"));
                }
                if let Some(run) = step.get("run") {
                    let run = run.as_str().ok_or("non-string run")?;
                    let dir = match step.get("working-directory") {
                        Some(value) => value.as_str().ok_or("non-string working-directory")?,
                        None => ".",
                    };
                    let id = if path.as_str() == ".github/workflows/ci-develop.yml" {
                        format!("{name}:{index}")
                    } else {
                        format!("{path}:{name}:{index}")
                    };
                    entries.push((id, run.to_owned(), dir.trim_matches('\'').to_owned()));
                } else if let Some(action) = step.get("uses").and_then(yaml_serde::Value::as_str) {
                    if action.starts_with("./") || !known_action(action) {
                        return Err(format!("unknown action: {action}"));
                    }
                } else {
                    return Err(format!("unknown step: {path}:{name}:{index}"));
                }
            }
        }
    }
    if !standard_seen || entries.is_empty() {
        return Err("standard CI or heavy entries missing".into());
    }
    Ok(entries)
}

fn known_action(action: &str) -> bool {
    [
        "actions/checkout@",
        "oven-sh/setup-bun@",
        "Swatinem/rust-cache@",
    ]
    .iter()
    .any(|prefix| {
        action
            .strip_prefix(prefix)
            .is_some_and(|sha| sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()))
    })
}

fn join(dir: &str, path: &str) -> Result<String, String> {
    let full = if dir == "." {
        PathBuf::from(path)
    } else {
        Path::new(dir).join(path)
    };
    if full
        .components()
        .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!("dynamic or unsafe path: {}", full.display()));
    }
    Ok(full
        .components()
        .filter_map(|part| match part {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/"))
}

fn command(
    tree: &Tree<'_>,
    entries: &mut BTreeSet<String>,
    run: &str,
    dir: &str,
    depth: usize,
) -> Result<(), String> {
    if depth > 16 {
        return Err("recursive script".into());
    }
    let run = run.trim().trim_matches('\'');
    if run.contains([
        '\n', ';', '|', '$', '`', '*', '?', '{', '}', '<', '>', '&', '\\',
    ]) {
        return Err(format!("dynamic shell: {run}"));
    }
    let words: Vec<_> = run.split_ascii_whitespace().collect();
    match words.as_slice() {
        ["echo", ..] | ["rustup", "show"] => Ok(()),
        ["bun", "run", name] => {
            let package_path = join(dir, "package.json")?;
            let package: Value =
                serde_json::from_str(&tree.read(&package_path)?).map_err(|e| e.to_string())?;
            let script = package
                .get("scripts")
                .and_then(|v| v.get(*name))
                .and_then(Value::as_str)
                .ok_or_else(|| format!("unknown package script: {name}"))?;
            entries.insert(format!("package:{package_path}:{name}"));
            command(tree, entries, script, dir, depth + 1)
        }
        ["bun", path] if path.ends_with(".ts") || path.ends_with(".js") => {
            let path = join(dir, path)?;
            entries.insert(format!("source:{path}"));
            source(tree, &path)
        }
        _ => Err(format!("unknown command: {run}")),
    }
}

fn source(tree: &Tree<'_>, path: &str) -> Result<(), String> {
    if path.ends_with(".rs") {
        return Err(format!("Rust source requires an adapter parser: {path}"));
    }
    let source = tree.read(path)?;
    if source.trim().is_empty() {
        return Err(format!("empty source: {path}"));
    }
    // This first supported source subset is deliberately tiny. Anything outside
    // it needs an adapter extension with a parser before it can be proven safe.
    for statement in source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if !statement
            .strip_prefix("console.log('")
            .and_then(|rest| rest.strip_suffix("');"))
            .is_some_and(|content| {
                content
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == ' ')
            })
        {
            return Err(format!("unresolved source read or syntax: {path}"));
        }
    }
    Ok(())
}

/// The supplied adapter set is only coverage evidence; the checker discovers
/// entries from tracked workflows and traverses their commands itself.
pub fn check(root: &Path, covered: &BTreeSet<String>) -> Result<BTreeSet<String>, String> {
    let tree = Tree {
        root,
        files: tracked(root)?,
    };
    for path in &tree.files {
        if EXECUTION_CONFIGS.iter().any(|(pattern, proof)| {
            pattern.matches(path) && matches!(proof, ConfigProof::Unsupported)
        }) {
            return Err(format!("unresolved execution configuration: {path}"));
        }
    }
    let declaration: Value = serde_json::from_str(&tree.read(".config/develop-tools.json")?)
        .map_err(|e| format!("invalid CI declaration: {e}"))?;
    let ci = declaration
        .get("ci")
        .and_then(Value::as_object)
        .ok_or("missing CI declaration")?;
    if ci.get("recordOnlySkip") != Some(&Value::Bool(true)) {
        return Err("record-only skip is not enabled".into());
    }
    if ci.get("bun").is_some_and(|value| !value.is_object())
        || ci.get("rust").is_some_and(|value| !value.is_object())
    {
        return Err("invalid CI check declaration".into());
    }
    let mut discovered = BTreeSet::new();
    let mut bun_check = false;
    let mut rust_job_seen = false;
    for (id, run, dir) in workflow_entries(&tree)? {
        if id.starts_with("rust:") {
            rust_job_seen = true;
        }
        if id.starts_with("bun:")
            && run.trim().trim_matches('\'').starts_with("bun run ")
            && !run.contains("ci:install")
        {
            bun_check = true;
        }
        discovered.insert(id);
        command(&tree, &mut discovered, &run, &dir, 0)?;
    }
    if ci.contains_key("bun") && !bun_check {
        return Err("declared Bun check missing".into());
    }
    if ci.contains_key("rust") && !rust_job_seen {
        return Err("declared Rust job missing".into());
    }
    for package in tree.files.iter().filter(|path| {
        EXECUTION_CONFIGS.iter().any(|(pattern, proof)| {
            pattern.matches(path) && matches!(proof, ConfigProof::PackageScripts)
        })
    }) {
        let value: Value = serde_json::from_str(&tree.read(package)?).map_err(|e| e.to_string())?;
        let fields = value.as_object().ok_or("non-object package.json")?;
        if fields.iter().any(|(key, value)| match key.as_str() {
            "name" | "version" | "type" => !value.is_string(),
            "private" => !value.is_boolean(),
            "scripts" => !value
                .as_object()
                .is_some_and(|scripts| scripts.values().all(Value::is_string)),
            _ => true,
        }) {
            return Err(format!("unresolved package configuration: {package}"));
        }
        if let Some(scripts) = value.get("scripts").and_then(Value::as_object) {
            let dir = package
                .strip_suffix("package.json")
                .unwrap_or("")
                .trim_end_matches('/');
            let dir = if dir.is_empty() { "." } else { dir };
            for (name, script) in scripts {
                discovered.insert(format!("package:{package}:{name}"));
                command(
                    &tree,
                    &mut discovered,
                    script.as_str().ok_or("non-string package script")?,
                    dir,
                    0,
                )?;
            }
        }
    }
    for cargo in tree
        .files
        .iter()
        .filter(|path| path.as_str() == "Cargo.toml" || path.ends_with("/Cargo.toml"))
    {
        let value: toml::Table = tree
            .read(cargo)?
            .parse()
            .map_err(|e| format!("invalid Cargo manifest: {e}"))?;
        if value.get("package").is_some() {
            if value.get("lib").is_some()
                || value.get("bin").is_some()
                || value.get("test").is_some()
                || value.get("example").is_some()
                || value.get("bench").is_some()
                || value
                    .get("package")
                    .and_then(toml::Value::as_table)
                    .is_some_and(|package| package.contains_key("build"))
            {
                return Err(format!("explicit Cargo target requires parser: {cargo}"));
            }
            let dir = cargo.strip_suffix("Cargo.toml").unwrap_or("");
            let target_paths = ["src/lib.rs", "src/main.rs", "build.rs"];
            for path in target_paths {
                let full = format!("{dir}{path}");
                if tree.files.contains(&full) {
                    discovered.insert(format!("rust:{full}"));
                    source(&tree, &full)?;
                }
            }
            for path in tree.files.iter().filter(|path| {
                path.starts_with(&format!("{dir}src/bin/"))
                    || path.starts_with(&format!("{dir}tests/"))
            }) {
                if path.ends_with(".rs") {
                    discovered.insert(format!("rust:{path}"));
                    source(&tree, path)?;
                }
            }
            if tree.files.iter().any(|path| {
                (path.starts_with(&format!("{dir}examples/"))
                    || path.starts_with(&format!("{dir}benches/")))
                    && path.ends_with(".rs")
            }) {
                return Err(format!("Cargo example or bench requires parser: {cargo}"));
            }
        }
    }
    if &discovered != covered {
        return Err(format!(
            "adapter coverage mismatch: discovered={discovered:?}"
        ));
    }
    Ok(discovered)
}
