//! Repo ごとの secret 出力宣言。入力の値は診断に含めない。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};

use crate::refs;

pub struct Declaration {
    pub targets: Vec<Target>,
    pub home_config: PathBuf,
}

pub struct Target {
    pub name: String,
    pub output: PathBuf,
    pub entries: BTreeMap<String, Entry>,
    pub optional: BTreeSet<String>,
    pub home_output: bool,
}

pub struct Entry {
    pub template: String,
    pub references: Vec<String>,
}

/// 宣言に `project` が無いときの失敗の符号（横断監査と同じ語）。
pub const MISSING_PROJECT: &str = "missing-project";

pub fn load(path: &Path, repo_root: Option<&Path>, home: &Path) -> Result<Declaration, String> {
    let repo_dir = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let repo = fs::canonicalize(repo_root.unwrap_or(repo_dir))
        .map_err(|_| "宣言の repo directory を読めません".to_owned())?;
    if !repo.is_dir() {
        return Err("宣言の repo root は directory にしてください".to_owned());
    }
    if repo_root.is_some() {
        let declaration = fs::canonicalize(path).map_err(|_| "宣言を読めません".to_owned())?;
        if !declaration.starts_with(&repo) {
            return Err("宣言は repo root の配下に置いてください".to_owned());
        }
    }
    let text = fs::read_to_string(path).map_err(|_| "宣言を読めません".to_owned())?;
    let jsonc = strip_jsonc(&text)?;
    let value: Value = serde_json::from_str(&jsonc).map_err(|error| {
        format!(
            "宣言の JSONC が不正です（{}:{}）",
            error.line(),
            error.column()
        )
    })?;
    let root = object(&value, "宣言")?;
    keys(root, &["version", "project", "targets"], "宣言")?;
    if root.get("version").and_then(Value::as_u64) != Some(1) {
        return Err("宣言の version は 1 にしてください".to_owned());
    }
    let targets = root
        .get("targets")
        .and_then(Value::as_array)
        .filter(|targets| !targets.is_empty())
        .ok_or("宣言の targets は空でない配列にしてください")?;
    if !root.contains_key("project") {
        return Err(format!(
            "{MISSING_PROJECT}: 宣言に project（vault の既定と ~/.config の dir 名）を書いてください"
        ));
    }
    let repo_name = required_string(root, "project", "宣言")?.to_owned();
    let home_config = home.join(".config").join(&repo_name);
    let mut names = BTreeSet::new();
    let mut outputs = BTreeSet::new();
    let mut parsed = Vec::new();
    for (index, value) in targets.iter().enumerate() {
        let label = format!("targets[{index}]");
        let target = object(value, &label)?;
        keys(
            target,
            &["name", "output", "template", "refs", "optional"],
            &label,
        )?;
        let name = required_string(target, "name", &label)?;
        if !names.insert(name.to_owned()) {
            return Err(format!("{label}: name が重複しています"));
        }
        let output_text = required_string(target, "output", &label)?;
        let (output, home_output) = output_path(output_text, &repo, &repo_name, &home_config)?;
        if !outputs.insert(output.clone()) {
            return Err(format!("{label}: output が重複しています"));
        }
        let mut entries = BTreeMap::new();
        if let Some(template) = target.get("template") {
            let template = template
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("{label}: template は文字列にしてください"))?;
            let template_path = safe_relative(template)
                .ok_or_else(|| format!("{label}: template path が不正です"))?;
            reject_template_symlinks(&repo, &template_path)
                .map_err(|message| format!("{label}: {message}"))?;
            let content = fs::read_to_string(repo.join(&template_path))
                .map_err(|_| format!("{label}: template を読めません"))?;
            for (line_number, line) in content.lines().enumerate() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let (key, raw) = line.split_once('=').ok_or_else(|| {
                    format!("{label}: template {} 行目の代入が不正です", line_number + 1)
                })?;
                if !refs::is_env_key(key) || entries.contains_key(key) {
                    return Err(format!(
                        "{label}: template {} 行目の key が不正または重複しています",
                        line_number + 1
                    ));
                }
                let raw = raw.trim();
                let raw = if raw.len() >= 2
                    && ((raw.starts_with('"') && raw.ends_with('"'))
                        || (raw.starts_with('\'') && raw.ends_with('\'')))
                {
                    &raw[1..raw.len() - 1]
                } else {
                    raw
                };
                let references = template_references(raw).map_err(|_| {
                    format!("{label}: template {} 行目の参照が不正です", line_number + 1)
                })?;
                entries.insert(
                    key.to_owned(),
                    Entry {
                        template: raw.to_owned(),
                        references,
                    },
                );
            }
        }
        if let Some(refs_value) = target.get("refs") {
            let refs_object = object(refs_value, &format!("{label}.refs"))?;
            for (key, value) in refs_object {
                if !refs::is_env_key(key) || entries.contains_key(key) {
                    return Err(format!(
                        "{label}: template と refs の key が重複または不正です: {key}"
                    ));
                }
                let reference = value
                    .as_str()
                    .ok_or_else(|| format!("{label}: {key} の参照が文字列ではありません"))?;
                let reference = normalized_reference(reference, &repo_name)
                    .ok_or_else(|| format!("{label}: {key} の参照が不正です"))?;
                entries.insert(
                    key.clone(),
                    Entry {
                        template: format!("{{{{ {reference} }}}}"),
                        references: vec![reference],
                    },
                );
            }
        }
        let mut optional = BTreeSet::new();
        if let Some(value) = target.get("optional") {
            let list = value
                .as_array()
                .ok_or_else(|| format!("{label}: optional は配列にしてください"))?;
            for key in list {
                let key = key
                    .as_str()
                    .ok_or_else(|| format!("{label}: optional の key は文字列にしてください"))?;
                if !entries.contains_key(key) || !optional.insert(key.to_owned()) {
                    return Err(format!(
                        "{label}: optional の key が不正または重複しています: {key}"
                    ));
                }
            }
        }
        parsed.push(Target {
            name: name.to_owned(),
            output,
            entries,
            optional,
            home_output,
        });
    }
    Ok(Declaration {
        targets: parsed,
        home_config,
    })
}

fn object<'a>(value: &'a Value, label: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{label} は object にしてください"))
}

fn keys(object: &Map<String, Value>, allowed: &[&str], label: &str) -> Result<(), String> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("{label}: 未知の key {key}"));
        }
    }
    Ok(())
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    label: &str,
) -> Result<&'a str, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{label}: {key} は空でない文字列にしてください"))
}

fn safe_relative(value: &str) -> Option<PathBuf> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    Some(path.to_owned())
}

fn output_path(
    value: &str,
    repo: &Path,
    repo_name: &str,
    home_config: &Path,
) -> Result<(PathBuf, bool), String> {
    if let Some(rest) = value.strip_prefix("~/") {
        let prefix = format!(".config/{repo_name}/");
        let child = rest
            .strip_prefix(&prefix)
            .and_then(safe_relative)
            .ok_or("output は ~/.config/<repo 名>/ の下だけ指定できます")?;
        let home = home_config
            .parent()
            .and_then(Path::parent)
            .ok_or("HOME が不正です")?;
        check_existing_parents(home, Path::new(rest), Some(&home.join(".config")))?;
        return Ok((home_config.join(child), true));
    }
    Ok((repo_output_path(repo, value)?, false))
}

/// 宣言 mode と stdin mode が共有する repo 内の出力先検査。
pub fn repo_output_path(repo: &Path, value: &str) -> Result<PathBuf, String> {
    let child =
        safe_relative(value).ok_or("output は repo 相対で、.. と絶対 path を含めないでください")?;
    check_existing_parents(repo, &child, None)?;
    Ok(repo.join(child))
}

enum ComponentKind {
    Missing,
    Symlink,
    Directory,
    Other,
}

fn component_kind(path: &Path) -> Result<ComponentKind, ()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Ok(ComponentKind::Symlink),
        Ok(meta) if meta.is_dir() => Ok(ComponentKind::Directory),
        Ok(_) => Ok(ComponentKind::Other),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ComponentKind::Missing),
        Err(_) => Err(()),
    }
}

/// template 自身と途中の directory が symlink なら、中身を読む前に拒否する。
fn reject_template_symlinks(base: &Path, relative: &Path) -> Result<(), String> {
    let mut current = base.to_owned();
    for component in relative.components() {
        current.push(component);
        match component_kind(&current) {
            Ok(ComponentKind::Symlink) => {
                return Err("template path に symlink があります".to_owned());
            }
            Ok(ComponentKind::Missing) => return Ok(()),
            Ok(ComponentKind::Directory | ComponentKind::Other) => {}
            Err(()) => return Err("template を読めません".to_owned()),
        }
    }
    Ok(())
}

fn check_existing_parents(
    base: &Path,
    relative: &Path,
    shared_config: Option<&Path>,
) -> Result<(), String> {
    let mut current = base.to_owned();
    if let Some(parent) = relative.parent() {
        for component in parent.components() {
            current.push(component);
            if shared_config == Some(current.as_path()) {
                match fs::metadata(&current) {
                    Ok(meta) if !meta.is_dir() => {
                        return Err("output の共有 config が directory ではありません".to_owned());
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err("output の共有 config を読めません".to_owned()),
                }
                continue;
            }
            match component_kind(&current) {
                Ok(ComponentKind::Symlink | ComponentKind::Other) => {
                    return Err(
                        "output の親 directory が symlink または非 directory です".to_owned()
                    );
                }
                Ok(ComponentKind::Directory | ComponentKind::Missing) => {}
                Err(()) => return Err("output の親 directory を読めません".to_owned()),
            }
        }
    }
    Ok(())
}

fn normalized_reference(value: &str, repo_name: &str) -> Option<String> {
    let reference = if let Some(rest) = value.strip_prefix("op://") {
        if rest.split('/').count() == 2 {
            format!("op://{repo_name}/{rest}")
        } else {
            value.to_owned()
        }
    } else {
        format!("op://{repo_name}/{value}")
    };
    let parts: Vec<_> = reference.strip_prefix("op://")?.split('/').collect();
    if !(3..=4).contains(&parts.len())
        || parts
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return None;
    }
    Some(reference)
}

fn template_references(raw: &str) -> Result<Vec<String>, ()> {
    let mut rest = raw;
    let mut refs = Vec::new();
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or(())?;
        let reference = after[..end].trim();
        if !reference.starts_with("op://") || normalized_reference(reference, "").is_none() {
            return Err(());
        }
        refs.push(reference.to_owned());
        rest = &after[end + 2..];
    }
    if rest.contains("}}") {
        return Err(());
    }
    Ok(refs)
}

/// JSONC の comments と末尾 comma だけを除く。文字列内の記号は保持する。
pub fn strip_jsonc(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = bytes.to_vec();
    let mut index = 0;
    let mut string = false;
    while index < bytes.len() {
        if string {
            if bytes[index] == b'\\' {
                index += 2;
                continue;
            }
            if bytes[index] == b'"' {
                string = false;
            }
        } else if bytes[index] == b'"' {
            string = true;
        } else if bytes[index] == b'/' && index + 1 < bytes.len() && bytes[index + 1] == b'/' {
            let start = index;
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            for byte in &mut out[start..index] {
                *byte = b' ';
            }
            continue;
        } else if bytes[index] == b'/' && index + 1 < bytes.len() && bytes[index + 1] == b'*' {
            let start = index;
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
                index += 1;
            }
            if index + 1 >= bytes.len() {
                return Err("JSONC の comment が閉じていません".to_owned());
            }
            index += 2;
            for byte in &mut out[start..index] {
                if *byte != b'\n' {
                    *byte = b' ';
                }
            }
            continue;
        }
        index += 1;
    }
    if string {
        return Err("JSONC の文字列が閉じていません".to_owned());
    }
    let mut string = false;
    let mut index = 0;
    while index < out.len() {
        if string {
            if out[index] == b'\\' {
                index += 2;
                continue;
            }
            if out[index] == b'"' {
                string = false;
            }
        } else if out[index] == b'"' {
            string = true;
        } else if out[index] == b',' {
            let mut next = index + 1;
            while next < out.len() && out[next].is_ascii_whitespace() {
                next += 1;
            }
            if next < out.len() && (out[next] == b']' || out[next] == b'}') {
                out[index] = b' ';
            }
        }
        index += 1;
    }
    String::from_utf8(out).map_err(|_| "JSONC は UTF-8 にしてください".to_owned())
}
