//! Repo が宣言した版 file の読み取りと、版の欄だけの書き換え。
use crate::Version;
use develop_tools_core::develop_tools_manifest;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    PackageJson,
    CargoWorkspace,
    CargoPackage,
    CargoLock,
    TomlRoot,
}

impl Kind {
    fn toml_location(self) -> Option<(&'static [&'static str], &'static str)> {
        match self {
            Self::CargoWorkspace => {
                Some((&["workspace", "package", "version"], "[workspace.package]"))
            }
            Self::CargoPackage => Some((&["package", "version"], "[package]")),
            Self::TomlRoot => Some((&["version"], "")),
            Self::PackageJson | Self::CargoLock => None,
        }
    }
}

/// 版 file の宣言 1 件。path は宣言した root からの相対。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub path: String,
    pub kind: Kind,
}

#[derive(Debug, Deserialize)]
struct Release {
    #[serde(rename = "versionFiles")]
    version_files: Vec<Entry>,
    /// 版 commit の中で root の CHANGELOG に刻印するか。CHANGELOG の慣行は repo の事実なので宣言で決める。
    #[serde(rename = "changelogStamp", default)]
    changelog_stamp: bool,
}

/// 刻印する CHANGELOG。root の 1 つだけを見る。
pub const CHANGELOG: &str = "CHANGELOG.md";

pub struct VersionFiles {
    files: Vec<(String, String, Kind)>,
    current: Version,
    changelog_stamp: bool,
}

fn read(root: &Path, relative: &str) -> Result<String, String> {
    std::fs::read_to_string(root.join(relative))
        .map_err(|err| format!("{relative} を読めません: {err}"))
}

/// Git tree は `./` や `..` を解決しないので、filesystem と同じ path を字句で正規化する。
fn tree_path(relative: &str) -> Result<String, String> {
    let mut parts: Vec<&str> = Vec::new();
    for component in Path::new(relative).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if parts.pop().is_some() => {}
            Component::Normal(part) => parts.push(
                part.to_str()
                    .ok_or_else(|| format!("不正な版 file path: {relative}"))?,
            ),
            _ => return Err(format!("repo の外を指す path です: {relative}")),
        }
    }
    Ok(parts.join("/"))
}

/// 版 file の読み元。作業木か、tag の指す commit。
#[derive(Clone, Copy)]
enum Source<'a> {
    Tree(&'a Path),
    Commit { root: &'a Path, commit: &'a str },
}

impl Source<'_> {
    fn read(self, relative: &str) -> Result<String, String> {
        match self {
            Self::Tree(root) => read(root, relative),
            Self::Commit { root, commit } => {
                let relative = tree_path(relative)?;
                let out = develop_tools_core::git(root)
                    .args(["cat-file", "blob", &format!("{commit}:{relative}")])
                    .output()
                    .map_err(|err| format!("git cat-file を起動できません: {err}"))?;
                if !out.status.success() {
                    return Err(format!(
                        "{commit} の {relative} を読めません: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    ));
                }
                String::from_utf8(out.stdout)
                    .map_err(|_| format!("{commit} の {relative} が UTF-8 ではありません"))
            }
        }
    }

    fn exists(self, relative: &str) -> Result<bool, String> {
        match self {
            Self::Tree(root) => Ok(root.join(relative).exists()),
            Self::Commit { root, commit } => develop_tools_core::git(root)
                .args([
                    "cat-file",
                    "-e",
                    &format!("{commit}:{}", tree_path(relative)?),
                ])
                .output()
                .map(|out| out.status.success())
                .map_err(|err| format!("git cat-file を起動できません: {err}")),
        }
    }
}

fn toml_value(text: &str, path: &str) -> Result<toml::Value, String> {
    toml::from_str(text).map_err(|err| format!("{path}: TOML が不正です: {err}"))
}

fn version_at(value: &toml::Value, keys: &[&str], path: &str) -> Result<Version, String> {
    let mut value = value;
    for key in keys {
        value = value
            .get(*key)
            .ok_or_else(|| format!("{path}: {} がありません", keys.join(".")))?;
    }
    value
        .as_str()
        .ok_or_else(|| format!("{path}: {} は文字列でなければなりません", keys.join(".")))?
        .parse()
        .map_err(|err: String| format!("{path}: {err}"))
}

fn package_version(text: &str, path: &str) -> Result<Version, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|err| format!("{path}: JSON が不正です: {err}"))?;
    value
        .get("version")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{path}: top-level version がありません"))?
        .parse()
        .map_err(|err: String| format!("{path}: {err}"))
}

fn lock_packages(
    source: Source,
    files: &[(String, String, Kind)],
) -> Result<HashSet<String>, String> {
    let mut packages = HashSet::new();
    for (path, text, kind) in files {
        match kind {
            Kind::CargoPackage => {
                let value = toml_value(text, path)?;
                let name = value
                    .get("package")
                    .and_then(|p| p.get("name"))
                    .and_then(toml::Value::as_str)
                    .ok_or_else(|| format!("{path}: package.name がありません"))?;
                packages.insert(name.to_owned());
            }
            Kind::CargoWorkspace => {
                let value = toml_value(text, path)?;
                let base = Path::new(path).parent().unwrap_or(Path::new(""));
                if let Some(name) = value
                    .get("package")
                    .filter(|p| {
                        p.get("version")
                            .and_then(|v| v.get("workspace"))
                            .and_then(toml::Value::as_bool)
                            == Some(true)
                    })
                    .and_then(|p| p.get("name"))
                    .and_then(toml::Value::as_str)
                {
                    packages.insert(name.to_owned());
                }
                let members = value
                    .get("workspace")
                    .and_then(|w| w.get("members"))
                    .and_then(toml::Value::as_array)
                    .ok_or_else(|| format!("{path}: workspace.members がありません"))?;
                for member in members {
                    let member = member
                        .as_str()
                        .ok_or_else(|| format!("{path}: workspace member が不正です"))?;
                    if member.contains(['*', '?', '[', ']']) {
                        return Err(format!(
                            "{path}: workspace member の glob は未対応です: {member}"
                        ));
                    }
                    let member_path = base.join(member).join("Cargo.toml");
                    let member_path = member_path
                        .to_str()
                        .ok_or_else(|| format!("{path}: member path が不正です"))?;
                    let member_text = source.read(member_path)?;
                    let manifest = toml_value(&member_text, member_path)?;
                    let package = manifest
                        .get("package")
                        .ok_or_else(|| format!("{member_path}: package がありません"))?;
                    let name = package
                        .get("name")
                        .and_then(toml::Value::as_str)
                        .ok_or_else(|| format!("{member_path}: package.name がありません"))?;
                    let inherited = package
                        .get("version")
                        .and_then(|v| v.get("workspace"))
                        .and_then(toml::Value::as_bool)
                        == Some(true);
                    if inherited {
                        packages.insert(name.to_owned());
                    }
                }
            }
            _ => {}
        }
    }
    Ok(packages)
}

fn lock_version(text: &str, path: &str, names: &HashSet<String>) -> Result<Version, String> {
    if names.is_empty() {
        return Err(format!("{path}: Cargo.toml の package 宣言がありません"));
    }
    let value = toml_value(text, path)?;
    let packages = value
        .get("package")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| format!("{path}: package がありません"))?;
    let mut found = HashSet::new();
    let mut first = None;
    for package in packages {
        let Some(name) = package.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        if !names.contains(name) {
            continue;
        }
        if package.get("source").is_some() {
            continue;
        }
        let version = version_at(package, &["version"], path)?;
        if let Some((other_name, other)) = &first {
            if version != *other {
                return Err(format!(
                    "{path}: {name} ({version}) と {other_name} ({other}) の版が一致しません"
                ));
            }
        } else {
            first = Some((name.to_owned(), version));
        }
        found.insert(name.to_owned());
    }
    if found != *names {
        return Err(format!(
            "{path}: 宣言した Cargo package が Cargo.lock にありません"
        ));
    }
    Ok(first.expect("names is not empty and all found").1)
}

impl VersionFiles {
    pub fn load(root: &Path) -> Result<Self, String> {
        Self::load_from(Source::Tree(root))
    }

    /// 作業木ではなく `commit` の tree から宣言と版 file を読む。`commit` は commit の SHA。
    pub fn load_at(root: &Path, commit: &str) -> Result<Self, String> {
        Self::load_from(Source::Commit { root, commit })
    }

    /// 作業木の宣言が読めて、`release.versionFiles` の欄を持たないときだけ true。
    /// `release` の他の欄は [`Self::load`] と同じ型で検証し、読めない・壊れた宣言は false にして
    /// 失敗の報告は [`Self::load`] に任せる。
    pub fn undeclared(root: &Path) -> bool {
        match Self::declared_release(Source::Tree(root)) {
            Ok((_, None)) => true,
            Ok((_, Some(mut release))) => match release.as_object_mut() {
                Some(object) if !object.contains_key("versionFiles") => {
                    object.insert(
                        "versionFiles".to_owned(),
                        serde_json::Value::Array(Vec::new()),
                    );
                    serde_json::from_value::<Release>(release).is_ok()
                }
                _ => false,
            },
            Err(_) => false,
        }
    }

    /// 版固定 file の path と、その `release` の欄を読む。
    fn declared_release(
        source: Source,
    ) -> Result<(&'static str, Option<serde_json::Value>), String> {
        // 作業木か commit の tree か、同じ読み元の中で置き場を決める。作業木は共有の探索と同じ判定
        // （壊れた symlink も在るとみなす）にする。
        let located = match source {
            Source::Tree(root) => develop_tools_manifest::locate(root)?,
            Source::Commit { .. } => develop_tools_manifest::locate_in(|path| source.exists(path))?,
        };
        let manifest = located.unwrap_or(develop_tools_manifest::PATH);
        let raw = source.read(manifest)?;
        let value: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|err| format!("{manifest}: JSON が不正です: {err}"))?;
        let declared = develop_tools_manifest::declaration(&value)
            .map_err(|err| format!("{manifest}: {err}"))?;
        Ok((manifest, declared.release))
    }

    fn load_from(source: Source) -> Result<Self, String> {
        let (manifest, release) = Self::declared_release(source)?;
        let release: Release = serde_json::from_value(release.unwrap_or_default())
            .map_err(|err| format!("{manifest}: release が不正です: {err}"))?;
        if release.version_files.is_empty() {
            return Err("release.versionFiles が空です".to_owned());
        }
        let mut files = Self::from_source(source, release.version_files)?;
        files.changelog_stamp = release.changelog_stamp;
        Ok(files)
    }

    /// 版固定 file を読まず、引数の宣言から読む。CHANGELOG は刻印しない。
    pub fn from_entries(root: &Path, entries: Vec<Entry>) -> Result<Self, String> {
        Self::from_source(Source::Tree(root), entries)
    }

    fn from_source(source: Source, entries: Vec<Entry>) -> Result<Self, String> {
        if entries.is_empty() {
            return Err("版 file の宣言が空です".to_owned());
        }
        let mut seen = HashSet::new();
        let mut files = Vec::new();
        for entry in entries {
            let relative = Path::new(&entry.path);
            if relative.as_os_str().is_empty()
                || relative.is_absolute()
                || relative
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
            {
                return Err(format!("不正な版 file path: {}", entry.path));
            }
            if !seen.insert(entry.path.clone()) {
                return Err(format!("版 file path が重複しています: {}", entry.path));
            }
            files.push((entry.path.clone(), source.read(&entry.path)?, entry.kind));
        }
        if source.exists("package.json")?
            && !files
                .iter()
                .any(|(p, _, k)| p == "package.json" && *k == Kind::PackageJson)
        {
            return Err("root の package.json が版 file に宣言されていません".to_owned());
        }
        let names = lock_packages(source, &files)?;
        let mut first: Option<(String, Version)> = None;
        for (path, text, kind) in &files {
            let version = match kind {
                Kind::PackageJson => package_version(text, path)?,
                Kind::CargoLock => lock_version(text, path, &names)?,
                _ => version_at(
                    &toml_value(text, path)?,
                    kind.toml_location().unwrap().0,
                    path,
                )?,
            };
            if let Some((other_path, other)) = &first {
                if version != *other {
                    return Err(format!(
                        "{path} の版 ({version}) が {other_path} の版 ({other}) と一致しません"
                    ));
                }
            } else {
                first = Some((path.clone(), version));
            }
        }
        Ok(Self {
            files,
            current: first.expect("not empty").1,
            changelog_stamp: false,
        })
    }

    pub fn current(&self) -> Version {
        self.current
    }

    pub fn write(&self, root: &Path, next: Version) -> Result<Vec<String>, String> {
        self.write_with_dependent_locks(root, &[], next)
    }

    /// 宣言した版 file に加えて、宣言の Cargo package に path で依存する別の workspace の
    /// Cargo.lock の、その package の行も同じ版にする。全 file の変換が済んでから書く。
    pub fn write_with_dependent_locks(
        &self,
        root: &Path,
        dependent_locks: &[PathBuf],
        next: Version,
    ) -> Result<Vec<String>, String> {
        let names = lock_packages(Source::Tree(root), &self.files)?;
        let mut changes = Vec::<(PathBuf, String)>::new();
        for (relative, text, kind) in &self.files {
            let updated = match kind {
                Kind::PackageJson => replace_json_version(text, next)?,
                Kind::CargoLock => replace_lock_versions(text, &names, self.current, next)?,
                _ => replace_toml_version(text, kind.toml_location().unwrap().1, next)?,
            };
            changes.push((root.join(relative), updated));
        }
        let mut written: Vec<String> = self.files.iter().map(|(path, _, _)| path.clone()).collect();
        // 宣言の無い repo では CHANGELOG を読まない。宣言があって file が無いのは宣言の誤りなので止める。
        if self.changelog_stamp {
            let (stamped, count) = crate::stamp_changelog(&read(root, CHANGELOG)?, next);
            if count > 0 {
                changes.push((root.join(CHANGELOG), stamped));
                written.push(CHANGELOG.to_owned());
            }
        }
        for lock in dependent_locks {
            let text = std::fs::read_to_string(lock)
                .map_err(|err| format!("{} を読めません: {err}", lock.display()))?;
            let updated = replace_dependent_lock_versions(&text, &names, self.current, next)
                .map_err(|err| format!("{}: {err}", lock.display()))?;
            changes.push((lock.clone(), updated));
            written.push(lock.display().to_string());
        }
        // 全 file の変換を先に済ませてから書く。
        for (path, text) in &changes {
            std::fs::write(path, text)
                .map_err(|err| format!("{} を書けません: {err}", path.display()))?;
        }
        Ok(written)
    }

    pub fn check_tags(&self, tags: &[String]) -> Result<Option<Version>, String> {
        let mut released = false;
        for tag in tags {
            let Some(raw) = tag.strip_prefix('v') else {
                continue;
            };
            let Ok(tagged) = raw.parse::<Version>() else {
                continue;
            };
            released = true;
            if tagged != self.current {
                return Err(format!(
                    "tag {tag} と宣言した file の版 ({}) が一致しません",
                    self.current
                ));
            }
        }
        Ok(released.then_some(self.current))
    }
}

fn replace_json_version(text: &str, version: Version) -> Result<String, String> {
    let span = crate::package_version_span(text)?;
    let mut next = text.to_owned();
    next.replace_range(span, &version.to_string());
    Ok(next)
}

fn replace_toml_version(text: &str, section: &str, version: Version) -> Result<String, String> {
    let mut active = section.is_empty();
    let mut found = false;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            active = trimmed
                .split_once('#')
                .map_or(trimmed, |(heading, _)| heading.trim_end())
                == section;
        }
        if active
            && trimmed.starts_with("version")
            && trimmed["version".len()..].trim_start().starts_with('=')
        {
            if found {
                return Err(format!("{section}: version が重複しています"));
            }
            let eq = line.find('=').expect("version line");
            let value = &line[eq + 1..];
            let quote = value
                .find('"')
                .ok_or_else(|| format!("{section}: version が文字列ではありません"))?;
            let end = value[quote + 1..]
                .find('"')
                .ok_or_else(|| format!("{section}: version が閉じていません"))?
                + quote
                + 1;
            out.push_str(&line[..eq + 1 + quote + 1]);
            out.push_str(&version.to_string());
            out.push_str(&value[end..]);
            found = true;
        } else {
            out.push_str(line);
        }
    }
    if !found {
        return Err(format!("{section}: version がありません"));
    }
    Ok(out)
}

fn replace_lock_versions(
    text: &str,
    names: &HashSet<String>,
    old: Version,
    next: Version,
) -> Result<String, String> {
    let (out, replaced) = replace_lock_sections(text, names, old, next)?;
    if replaced != *names {
        return Err("Cargo.lock の対象 package が不足しています".to_owned());
    }
    Ok(out)
}

fn replace_lock_sections(
    text: &str,
    names: &HashSet<String>,
    old: Version,
    next: Version,
) -> Result<(String, HashSet<String>), String> {
    let mut out = String::with_capacity(text.len());
    let mut section = String::new();
    let mut replaced = HashSet::new();
    for line in text.split_inclusive('\n') {
        if line.trim() == "[[package]]" {
            if !section.is_empty() {
                out.push_str(&replace_lock_section(
                    &section,
                    names,
                    old,
                    next,
                    &mut replaced,
                )?);
            }
            section.clear();
            section.push_str(line);
        } else if section.is_empty() {
            out.push_str(line);
        } else {
            section.push_str(line);
        }
    }
    if !section.is_empty() {
        out.push_str(&replace_lock_section(
            &section,
            names,
            old,
            next,
            &mut replaced,
        )?);
    }
    Ok((out, replaced))
}

/// 依存する側の Cargo.lock は宣言の package の一部だけを持つことがあるので、全部は求めない。
/// 1 つも無いときと前の版と食い違うときは、依存の宣言か lock が誤っているので止める。
fn replace_dependent_lock_versions(
    text: &str,
    names: &HashSet<String>,
    old: Version,
    next: Version,
) -> Result<String, String> {
    let value = toml_value(text, "Cargo.lock")?;
    let packages = value
        .get("package")
        .and_then(toml::Value::as_array)
        .ok_or("Cargo.lock: package がありません")?;
    let mut present = HashSet::new();
    for package in packages {
        let Some(name) = package.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        if !names.contains(name) || package.get("source").is_some() {
            continue;
        }
        let version = version_at(package, &["version"], "Cargo.lock")?;
        if version != old {
            return Err(format!(
                "{name} の版 ({version}) が宣言の版 ({old}) と一致しません"
            ));
        }
        present.insert(name.to_owned());
    }
    if present.is_empty() {
        return Err("宣言した Cargo package がありません".to_owned());
    }
    Ok(replace_lock_sections(text, &present, old, next)?.0)
}

fn replace_lock_section(
    section: &str,
    names: &HashSet<String>,
    old: Version,
    next: Version,
    replaced: &mut HashSet<String>,
) -> Result<String, String> {
    let value = toml_value(section, "Cargo.lock")?;
    let package = value
        .get("package")
        .and_then(toml::Value::as_array)
        .and_then(|p| p.first())
        .ok_or("Cargo.lock: package がありません")?;
    let name = package
        .get("name")
        .and_then(toml::Value::as_str)
        .ok_or("Cargo.lock: name がありません")?;
    if !names.contains(name) {
        return Ok(section.to_owned());
    }
    if package.get("source").is_some() {
        return Ok(section.to_owned());
    }
    if version_at(package, &["version"], "Cargo.lock")? != old {
        return Ok(section.to_owned());
    }
    replaced.insert(name.to_owned());
    replace_toml_version(section, "[[package]]", next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(major: u64, minor: u64, patch: u64) -> Version {
        Version {
            major,
            minor,
            patch,
        }
    }

    fn declare(root: &Path, entries: &str) {
        std::fs::write(
            root.join(".develop-tools.json"),
            format!(r#"{{"release":{{"versionFiles":[{entries}]}}}}"#),
        )
        .unwrap();
    }

    #[test]
    fn cargo_versions_change_only_declared_version_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("crates/app")).unwrap();
        let cargo = "# reason\n[workspace]\nmembers = [\"crates/app\"]\n[workspace.package] # x\nversion = \"0.1.0\" # keep\nedition = \"2021\"\n";
        let member = "[package]\nname = \"app\"\nversion.workspace = true\n";
        let lock = "version = 3\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\" # keep\n\n[[package]]\nname = \"dep\"\nversion = \"9.8.7\"\n";
        std::fs::write(root.join("Cargo.toml"), cargo).unwrap();
        std::fs::write(root.join("crates/app/Cargo.toml"), member).unwrap();
        std::fs::write(root.join("Cargo.lock"), lock).unwrap();
        declare(
            root,
            r#"{"path":"Cargo.toml","kind":"cargo-workspace"},{"path":"Cargo.lock","kind":"cargo-lock"}"#,
        );
        VersionFiles::load(root)
            .unwrap()
            .write(root, version(0, 1, 1))
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("Cargo.toml")).unwrap(),
            cargo.replace("version = \"0.1.0\"", "version = \"0.1.1\"")
        );
        assert_eq!(
            std::fs::read_to_string(root.join("Cargo.lock")).unwrap(),
            lock.replace("version = \"0.1.0\"", "version = \"0.1.1\"")
        );
        assert_eq!(
            std::fs::read_to_string(root.join("crates/app/Cargo.toml")).unwrap(),
            member
        );
    }

    #[test]
    fn independently_versioned_workspace_root_is_not_a_lock_target() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("member")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"root\"\nversion = \"9.0.0\"\n[workspace]\nmembers = [\"member\"]\n[workspace.package]\nversion = \"0.1.0\"\n").unwrap();
        std::fs::write(
            root.join("member/Cargo.toml"),
            "[package]\nname = \"member\"\nversion.workspace = true\n",
        )
        .unwrap();
        std::fs::write(root.join("Cargo.lock"), "version = 3\n\n[[package]]\nname = \"root\"\nversion = \"9.0.0\"\n\n[[package]]\nname = \"member\"\nversion = \"0.1.0\"\n").unwrap();
        declare(
            root,
            r#"{"path":"Cargo.toml","kind":"cargo-workspace"},{"path":"Cargo.lock","kind":"cargo-lock"}"#,
        );
        VersionFiles::load(root)
            .unwrap()
            .write(root, version(0, 1, 1))
            .unwrap();
        let lock = std::fs::read_to_string(root.join("Cargo.lock")).unwrap();
        assert!(lock.contains("name = \"root\"\nversion = \"9.0.0\""));
        assert!(lock.contains("name = \"member\"\nversion = \"0.1.1\""));
    }

    #[test]
    fn glob_workspace_members_are_rejected_explicitly() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n[workspace.package]\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        declare(root, r#"{"path":"Cargo.toml","kind":"cargo-workspace"}"#);
        assert!(VersionFiles::load(root).err().unwrap().contains("glob"));
    }

    #[test]
    fn invalid_declarations_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        declare(root, "");
        assert!(VersionFiles::load(root).err().unwrap().contains("空"));
        std::fs::write(root.join("package.json"), "{\"version\":\"0.1.0\"}").unwrap();
        for (entries, error) in [
            (
                r#"{"path":"package.json","kind":"package-json"},{"path":"package.json","kind":"package-json"}"#,
                "重複",
            ),
            (
                r#"{"path":"/tmp/package.json","kind":"package-json"}"#,
                "不正な版 file path",
            ),
        ] {
            declare(root, entries);
            assert!(
                VersionFiles::load(root).err().unwrap().contains(error),
                "{entries}"
            );
        }
        std::fs::write(
            root.join(".develop-tools.json"),
            r#"{"platforms":["linux"],"release":{"versionFiles":[{"path":"package.json","kind":"package-json"}]}}"#,
        )
        .unwrap();
        assert!(
            VersionFiles::load(root)
                .err()
                .unwrap()
                .contains("preflightWorkflow")
        );
    }

    fn git(root: &Path, args: &[&str]) -> String {
        let out = develop_tools_core::git(root).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// 作業木と commit の tree は、それぞれの中で版固定 file の置き場を決める。
    #[test]
    fn manifest_is_located_within_each_source() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        git(root, &["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("package.json"), r#"{"version":"0.2.0"}"#).unwrap();
        std::fs::create_dir(root.join(".config")).unwrap();
        let manifest =
            r#"{"release":{"versionFiles":[{"path":"package.json","kind":"package-json"}]}}"#;
        std::fs::write(root.join(develop_tools_manifest::PATH), manifest).unwrap();
        git(root, &["add", "-A"]);
        git(
            root,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "moved",
            ],
        );
        let moved = git(root, &["rev-parse", "HEAD"]);
        // 作業木だけを旧い置き場へ戻しても、commit の tree は移した置き場を読む。
        std::fs::rename(
            root.join(develop_tools_manifest::PATH),
            root.join(develop_tools_manifest::LEGACY_PATH),
        )
        .unwrap();
        assert_eq!(VersionFiles::load(root).unwrap().current, version(0, 2, 0));
        assert_eq!(
            VersionFiles::load_at(root, &moved).unwrap().current,
            version(0, 2, 0)
        );
        std::fs::write(root.join(develop_tools_manifest::PATH), manifest).unwrap();
        let both = VersionFiles::load(root).err().unwrap();
        assert!(both.starts_with("manifest_duplicated"), "{both}");
        #[cfg(unix)]
        {
            // 新しい置き場が壊れた symlink でも、作業木では在るとみなして止める（共有の探索と同じ判定）。
            std::fs::remove_file(root.join(develop_tools_manifest::PATH)).unwrap();
            std::os::unix::fs::symlink("missing.json", root.join(develop_tools_manifest::PATH))
                .unwrap();
            let dangling = VersionFiles::load(root).err().unwrap();
            assert!(dangling.starts_with("manifest_duplicated"), "{dangling}");
            std::fs::remove_file(root.join(develop_tools_manifest::PATH)).unwrap();
            std::fs::write(root.join(develop_tools_manifest::PATH), manifest).unwrap();
        }
        git(root, &["add", "-A"]);
        git(
            root,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "both",
            ],
        );
        let both = VersionFiles::load_at(root, "HEAD").err().unwrap();
        assert!(both.starts_with("manifest_duplicated"), "{both}");
    }

    fn cli_workspace(root: &Path) -> VersionFiles {
        for member in ["core", "extra"] {
            std::fs::create_dir_all(root.join(member)).unwrap();
            std::fs::write(
                root.join(member).join("Cargo.toml"),
                format!("[package]\nname = \"{member}\"\nversion.workspace = true\n"),
            )
            .unwrap();
        }
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"core\", \"extra\"]\n[workspace.package]\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        VersionFiles::from_entries(
            root,
            vec![Entry {
                path: "Cargo.toml".to_owned(),
                kind: Kind::CargoWorkspace,
            }],
        )
        .unwrap()
    }

    #[test]
    fn dependent_lock_rewrites_only_the_members_it_holds() {
        let dir = tempfile::tempdir().unwrap();
        let files = cli_workspace(dir.path());
        let lock = dir.path().join("dependent.lock");
        let text = "version = 3\n\n[[package]]\nname = \"core\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"extra\"\nversion = \"0.0.1\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n[[package]]\nname = \"tool\"\nversion = \"0.1.0\"\n";
        std::fs::write(&lock, text).unwrap();
        files
            .write_with_dependent_locks(dir.path(), std::slice::from_ref(&lock), version(0, 1, 1))
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&lock).unwrap(),
            text.replacen("version = \"0.1.0\"", "version = \"0.1.1\"", 1)
        );
    }

    #[test]
    fn dependent_lock_that_disagrees_stops_before_any_write() {
        let dir = tempfile::tempdir().unwrap();
        let files = cli_workspace(dir.path());
        let cargo = std::fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
        let lock = dir.path().join("dependent.lock");
        for (text, error) in [
            (
                "version = 3\n\n[[package]]\nname = \"core\"\nversion = \"0.0.9\"\n",
                "一致しません",
            ),
            (
                "version = 3\n\n[[package]]\nname = \"tool\"\nversion = \"0.1.0\"\n",
                "宣言した Cargo package がありません",
            ),
        ] {
            std::fs::write(&lock, text).unwrap();
            let err = files
                .write_with_dependent_locks(
                    dir.path(),
                    std::slice::from_ref(&lock),
                    version(0, 1, 1),
                )
                .unwrap_err();
            assert!(err.contains(error), "{err}");
            assert_eq!(std::fs::read_to_string(&lock).unwrap(), text);
            assert_eq!(
                std::fs::read_to_string(dir.path().join("Cargo.toml")).unwrap(),
                cargo
            );
        }
    }
}
