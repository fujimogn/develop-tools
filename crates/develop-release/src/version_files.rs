//! Repo が宣言した版 file の読み取りと、版の欄だけの書き換え。
use crate::Version;
use develop_tools_core::develop_tools_manifest;
use develop_tools_core::release_tag::TagPrefix;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    PackageJson,
    CargoWorkspace,
    CargoPackage,
    CargoLock,
    CargoLockSubset,
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
            Self::PackageJson | Self::CargoLock | Self::CargoLockSubset => None,
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

// Public Entry literals remain usable by the independent CLI publication path.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredEntry {
    path: String,
    kind: Kind,
    #[serde(default, deserialize_with = "manifest_field")]
    manifest: Option<String>,
}

fn manifest_field<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

impl From<Entry> for DeclaredEntry {
    fn from(entry: Entry) -> Self {
        Self {
            path: entry.path,
            kind: entry.kind,
            manifest: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Release {
    #[serde(rename = "versionFiles")]
    version_files: Vec<DeclaredEntry>,
}

/// 刻印する CHANGELOG。root の 1 つだけを見る。
pub const CHANGELOG: &str = "CHANGELOG.md";

pub struct VersionFiles {
    files: Vec<(String, String, Kind)>,
    current: Version,
    cargo_names: HashSet<String>,
    subsets: HashMap<String, HashSet<String>>,
    tag_prefix: TagPrefix,
    changelog_stamp: bool,
    notes: Option<crate::release_notes::DeclaredNotes>,
}

/// 1つの送信 commit の宣言。対象の判定と版の照合で同じ読取結果を使う。
pub struct TagSource<'a> {
    source: Source<'a>,
    declaration: Option<(&'static str, serde_json::Value)>,
    tag_prefix: TagPrefix,
}

impl<'a> TagSource<'a> {
    pub fn at(root: &'a Path, commit: &'a str) -> Result<Self, String> {
        let source = Source::Commit { root, commit };
        let declaration = develop_tools_manifest::locate_tag_in(|path| {
            let exists = source.exists(path)?;
            // A non-tree parent hides its child in ls-tree; it is not a pre-declaration commit.
            if !exists
                && let Some(parent) = Path::new(path)
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
            {
                let parent = parent.to_string_lossy();
                if source.exists(&parent)? {
                    source.check_path_type(&parent, true)?;
                }
            }
            Ok(exists)
        })?
        .map(|manifest| {
            VersionFiles::read_declaration(source, manifest).map(|value| (manifest, value))
        })
        .transpose()?;
        let tag_prefix = match &declaration {
            Some((manifest, value)) => develop_tools_manifest::release_tag_prefix(value)
                .map_err(|error| format!("{manifest}: {error}"))?,
            None => TagPrefix::default(),
        };
        Ok(Self {
            source,
            declaration,
            tag_prefix,
        })
    }

    pub fn parse_tag(&self, tag: &str) -> Option<Version> {
        self.tag_prefix.parse(tag)
    }

    pub fn versions(self) -> Result<VersionFiles, String> {
        match self.declaration {
            Some((manifest, value)) => {
                let declared = develop_tools_manifest::declaration(&value)
                    .map_err(|error| format!("{manifest}: {error}"))?;
                VersionFiles::from_declaration(self.source, manifest, declared)
            }
            None => {
                // 宣言以前の tag の数字だけを読む。現在の作業木にはこの互換を適用しない。
                let path = "Cargo.toml";
                let text = self.source.read(path)?;
                let current = version_at(
                    &toml_value(&text, path)?,
                    &["workspace", "package", "version"],
                    path,
                )?;
                Ok(VersionFiles {
                    files: vec![(path.to_owned(), text, Kind::CargoWorkspace)],
                    current,
                    cargo_names: HashSet::new(),
                    subsets: HashMap::new(),
                    tag_prefix: TagPrefix::default(),
                    changelog_stamp: false,
                    notes: None,
                })
            }
        }
    }
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

fn read_batch_pipe(mut pipe: impl std::io::Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn reap_batch_child(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

// Child ownership remains with the caller while pipe I/O is collected.
fn batch_output(
    child: &mut std::process::Child,
    requests: String,
    stdout: impl std::io::Read + Send,
) -> Option<std::process::Output> {
    use std::io::Write;
    let mut input = child.stdin.take().expect("piped batch stdin");
    let stderr = child.stderr.take().expect("piped batch stderr");
    std::thread::scope(|scope| {
        let (done, completed) = std::sync::mpsc::channel();
        let input_done = done.clone();
        let writer = scope.spawn(move || {
            let result = input.write_all(requests.as_bytes());
            let _ = input_done.send(result.is_ok());
            result
        });
        let output_done = done.clone();
        let output = scope.spawn(move || {
            let result = read_batch_pipe(stdout);
            let _ = output_done.send(result.is_ok());
            result
        });
        let errors = scope.spawn(move || {
            let result = read_batch_pipe(stderr);
            let _ = done.send(result.is_ok());
            result
        });
        let complete = (0..3).all(|_| completed.recv() == Ok(true));
        if !complete {
            // Closing the child's pipe ends unblocks both writer and readers.
            // Reap before joining: an I/O error must not leave a live child.
            reap_batch_child(child);
        }
        writer.join().ok()?.ok()?;
        let stdout = output.join().ok()?.ok()?;
        let stderr = errors.join().ok()?.ok()?;
        if !complete {
            return None;
        }
        let status = match child.wait() {
            Ok(status) => status,
            Err(_) => {
                reap_batch_child(child);
                return None;
            }
        };
        status.success().then_some(std::process::Output {
            status,
            stdout,
            stderr,
        })
    })
}

struct TreeEntry<'a> {
    mode: &'a str,
    object_type: &'a str,
    oid: &'a str,
    path: &'a str,
}

impl<'a> TreeEntry<'a> {
    fn parse(entry: &'a str) -> Option<Self> {
        let (metadata, path) = entry.split_once('\t')?;
        let mut fields = metadata.split(' ');
        let entry = Self {
            mode: fields.next()?,
            object_type: fields.next()?,
            oid: fields.next()?,
            path,
        };
        (fields.next().is_none() && !entry.oid.is_empty()).then_some(entry)
    }

    fn regular_blob(&self) -> bool {
        matches!(self.mode, "100644" | "100755") && self.object_type == "blob"
    }

    fn directory(&self) -> bool {
        self.mode == "040000" && self.object_type == "tree"
    }
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
                let relative = self.regular_path(relative)?;
                let out = develop_tools_core::git(root)
                    .args(["cat-file", "blob", &format!("{commit}:{relative}")])
                    .output()
                    .map_err(|err| format!("source_read_failed: git cat-file を起動できません: {err}。Git と commit を確認してください"))?;
                if !out.status.success() {
                    return Err(format!(
                        "source_read_failed: {commit} の {relative} を読めません: {}。Git と commit を確認してください",
                        develop_tools_core::process::ChildFailure::from_output(&out)
                    ));
                }
                String::from_utf8(out.stdout)
                    .map_err(|_| format!("source_read_failed: {commit} の {relative} が UTF-8 ではありません。commit 内の file を確認してください"))
            }
        }
    }

    // Read only the requested paths. Mode checks remain mandatory, and blob IDs
    // come from this commit's tree, never HEAD or the worktree.
    fn read_many(self, paths: &[String]) -> Option<Vec<String>> {
        use std::process::Stdio;
        let Self::Commit { root, commit } = self else {
            return None;
        };
        let normalized = paths
            .iter()
            .map(|path| self.normalized_path(path))
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        let listing = develop_tools_core::git(root)
            .args(["--literal-pathspecs", "ls-tree", "-z", commit, "--"])
            .args(&normalized)
            .output()
            .ok()?;
        if !listing.status.success() {
            return None;
        }
        let mut entries = HashMap::new();
        for entry in std::str::from_utf8(&listing.stdout)
            .ok()?
            .split_terminator('\0')
        {
            let entry = TreeEntry::parse(entry)?;
            if !entry.regular_blob() {
                return None;
            }
            entries.insert(entry.path, entry.oid);
        }
        let objects = normalized
            .iter()
            .map(|path| entries.get(path.as_str()).copied())
            .collect::<Option<Vec<_>>>()?;
        let requests = objects
            .iter()
            .map(|oid| format!("{oid}\n"))
            .collect::<String>();
        let mut child = develop_tools_core::git(root)
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?;
        let stdout = child.stdout.take().expect("piped batch stdout");
        let output = batch_output(&mut child, requests, stdout)?;
        let mut bytes = output.stdout.as_slice();
        let mut texts = Vec::with_capacity(paths.len());
        for oid in objects {
            let newline = bytes.iter().position(|byte| *byte == b'\n')?;
            let header = std::str::from_utf8(&bytes[..newline]).ok()?;
            let mut fields = header.split_whitespace();
            if fields.next()? != oid || fields.next()? != "blob" {
                return None;
            }
            let size: usize = fields.next()?.parse().ok()?;
            if fields.next().is_some() {
                return None;
            }
            bytes = bytes.get(newline + 1..)?;
            texts.push(std::str::from_utf8(bytes.get(..size)?).ok()?.to_owned());
            if *bytes.get(size)? != b'\n' {
                return None;
            }
            bytes = bytes.get(size.checked_add(1)?..)?;
        }
        bytes.is_empty().then_some(texts)
    }

    fn normalized_path(self, relative: &str) -> Result<String, String> {
        let mut parts = Vec::new();
        for component in Path::new(relative).components() {
            match component {
                Component::CurDir => {}
                Component::Normal(part) => parts.push(part.to_str().ok_or_else(|| format!("source_path_invalid: {relative} が UTF-8 ではありません。宣言を確認してください"))?),
                Component::ParentDir if !parts.is_empty() => {
                    // A cancelled prefix must really be a directory, not a symlink or gitlink.
                    self.check_path_type(&parts.join("/"), true)?;
                    parts.pop();
                }
                _ => return Err(format!("source_path_invalid: repo の外を指す path です: {relative}。宣言を確認してください")),
            }
        }
        Ok(parts.join("/"))
    }

    fn regular_path(self, relative: &str) -> Result<String, String> {
        let relative = self.normalized_path(relative)?;
        self.check_path_type(&relative, false)?;
        Ok(relative)
    }

    fn check_path_type(self, relative: &str, directory: bool) -> Result<(), String> {
        match self {
            Self::Tree(root) => {
                let mut path = root.to_owned();
                let mut file_type = None;
                for component in Path::new(relative).components() {
                    path.push(component);
                    let metadata = std::fs::symlink_metadata(&path)
                        .map_err(|error| format!("source_path_invalid: {relative} を読めません: {error}。宣言と checkout を確認してください"))?;
                    if metadata.file_type().is_symlink() {
                        return Err(format!(
                            "source_path_invalid: {relative} は symlink を通ります。repo 内の通常 path を指定して checkout を確認してください"
                        ));
                    }
                    file_type = Some(metadata.file_type());
                }
                if !file_type.is_some_and(|kind| {
                    if directory {
                        kind.is_dir()
                    } else {
                        kind.is_file()
                    }
                }) {
                    return Err(format!(
                        "source_path_invalid: {relative} の file/directory の種別が不正です。checkout を確認してください"
                    ));
                }
            }
            Self::Commit { root, commit } => {
                let listing = develop_tools_core::git(root)
                    .args(["--literal-pathspecs", "ls-tree", "-z", commit, "--", relative])
                    .output()
                    .map_err(|error| format!("source_query_failed: {commit} の {relative} を検査できません: {error}。Git と commit を確認してください"))?;
                if !listing.status.success() {
                    return Err(format!(
                        "source_query_failed: git ls-tree で {commit} の {relative} を検査できません: {}。Git と commit を確認してください",
                        develop_tools_core::process::ChildFailure::from_output(&listing)
                    ));
                }
                let entry = std::str::from_utf8(&listing.stdout)
                    .ok()
                    .and_then(|entry| entry.strip_suffix('\0'))
                    .and_then(TreeEntry::parse);
                let expected = entry.is_some_and(|entry| {
                    entry.path == relative
                        && if directory {
                            entry.directory()
                        } else {
                            entry.regular_blob()
                        }
                });
                if !expected {
                    return Err(format!(
                        "source_path_invalid: {commit} の {relative} の blob/tree の種別が不正です（symlink・submodule・欠落等）。宣言と commit の tree を確認してください"
                    ));
                }
            }
        }
        Ok(())
    }

    fn read_regular(self, relative: &str) -> Result<String, String> {
        match self {
            Self::Tree(_) => self.read(&self.regular_path(relative)?),
            Self::Commit { .. } => self.read(relative),
        }
    }

    fn exists(self, relative: &str) -> Result<bool, String> {
        match self {
            Self::Tree(root) => match std::fs::symlink_metadata(root.join(relative)) {
                Ok(_) => Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(format!(
                    "source_path_invalid: {relative} の存在を検査できません: {error}。宣言と checkout を確認してください"
                )),
            },
            Self::Commit { root, commit } => develop_tools_core::git(root)
                // tree 内の存在と blob の読取可否を分け、読めない宣言を「宣言以前」に倒さない。
                .args([
                    "--literal-pathspecs",
                    "ls-tree",
                    "--name-only",
                    commit,
                    "--",
                    &tree_path(relative)?,
                ])
                .output()
                .map_err(|err| format!("git ls-tree を起動できません: {err}"))
                .and_then(|out| {
                    if out.status.success() {
                        Ok(!out.stdout.is_empty())
                    } else {
                        Err(format!(
                            "{commit} の {relative} の存在を照合できません: {}",
                            String::from_utf8_lossy(&out.stderr).trim()
                        ))
                    }
                }),
        }
    }
}

struct Reader<'a> {
    source: Source<'a>,
    regular: bool,
    texts: HashMap<String, String>,
}

impl Reader<'_> {
    fn prefetch(&mut self, paths: impl IntoIterator<Item = String>) {
        if self.regular || !matches!(self.source, Source::Commit { .. }) {
            return;
        }
        let paths = paths
            .into_iter()
            .filter(|path| !self.texts.contains_key(path))
            .collect::<Vec<_>>();
        if paths.len() < 2 {
            return;
        }
        if let Some(texts) = self.source.read_many(&paths) {
            self.texts.extend(paths.into_iter().zip(texts));
        }
        // On any batch failure, leave the cache empty. Individual reads retain
        // the established error order, codes and mode checks; no file is skipped.
    }

    fn read(&mut self, path: &str) -> Result<String, String> {
        let path = if self.regular {
            self.source.normalized_path(path)?
        } else {
            path.to_owned()
        };
        if let Some(text) = self.texts.get(&path) {
            return Ok(text.clone());
        }
        let text = if self.regular {
            self.source.read_regular(&path)?
        } else {
            self.source.read(&path)?
        };
        self.texts.insert(path, text.clone());
        Ok(text)
    }
}

#[derive(Default)]
struct CargoPackages {
    names: HashSet<String>,
    workspace: HashMap<String, String>,
}

impl CargoPackages {
    fn workspace_member(
        &mut self,
        path: &str,
        value: &toml::Value,
        strict: bool,
    ) -> Result<(), String> {
        let Some(package) = value.get("package") else {
            return Ok(());
        };
        if package
            .get("version")
            .and_then(|v| v.get("workspace"))
            .and_then(toml::Value::as_bool)
            != Some(true)
        {
            return Ok(());
        }
        let name = package.get("name").and_then(toml::Value::as_str)
            .ok_or_else(|| format!("subset_package_invalid: {path}: package.name がありません。manifest を確認してください"))?;
        if strict && self.workspace.values().any(|other| other == name) {
            return Err(format!(
                "subset_package_ambiguous: {path}: {name} の manifest が重複しています。workspace.members を確認してください"
            ));
        }
        self.names.insert(name.to_owned());
        self.workspace.insert(path.to_owned(), name.to_owned());
        Ok(())
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
    reader: &mut Reader,
    files: &[(String, String, Kind)],
) -> Result<CargoPackages, String> {
    let mut packages = CargoPackages::default();
    for (path, text, kind) in files {
        match kind {
            Kind::CargoPackage => {
                let value = toml_value(text, path)?;
                let name = value
                    .get("package")
                    .and_then(|p| p.get("name"))
                    .and_then(toml::Value::as_str)
                    .ok_or_else(|| format!("{path}: package.name がありません"))?;
                packages.names.insert(name.to_owned());
            }
            Kind::CargoWorkspace => {
                let value = toml_value(text, path)?;
                let base = Path::new(path).parent().unwrap_or(Path::new(""));
                packages.workspace_member(path, &value, reader.regular)?;
                let members = value
                    .get("workspace")
                    .and_then(|w| w.get("members"))
                    .and_then(toml::Value::as_array)
                    .ok_or_else(|| format!("{path}: workspace.members がありません"))?;
                let members = members
                    .iter()
                    .map(|member| {
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
                        Ok(member_path.to_owned())
                    })
                    .collect::<Vec<Result<String, String>>>();
                reader.prefetch(
                    members
                        .iter()
                        .filter_map(|path| path.as_ref().ok())
                        .cloned(),
                );
                for member_path in members {
                    let member_path = member_path?;
                    let member_path = member_path.as_str();
                    let normalized;
                    let member_path = if reader.regular {
                        normalized = reader.source.normalized_path(member_path)?;
                        normalized.as_str()
                    } else {
                        member_path
                    };
                    let member_text = reader.read(member_path)?;
                    let manifest = toml_value(&member_text, member_path)?;
                    let package = manifest
                        .get("package")
                        .ok_or_else(|| format!("{member_path}: package がありません"))?;
                    package
                        .get("name")
                        .and_then(toml::Value::as_str)
                        .ok_or_else(|| format!("{member_path}: package.name がありません"))?;
                    packages.workspace_member(member_path, &manifest, reader.regular)?;
                }
            }
            _ => {}
        }
    }
    Ok(packages)
}

fn subset_targets(
    reader: &mut Reader,
    files: &[(String, String, Kind)],
    manifests: &HashMap<String, String>,
    packages: &HashMap<String, String>,
) -> Result<HashMap<String, HashSet<String>>, String> {
    let (workspace_path, _, _) = files
        .iter()
        .find(|(_, _, kind)| *kind == Kind::CargoWorkspace)
        .ok_or("subset_workspace_missing: cargo-workspace の宣言が必要です")?;
    let main_lock = Path::new(workspace_path).with_file_name("Cargo.lock");
    if !files
        .iter()
        .any(|(path, _, kind)| *kind == Kind::CargoLock && Path::new(path) == main_lock)
    {
        return Err(
            "subset_main_lock_missing: 本体 workspace の cargo-lock を宣言してください".to_owned(),
        );
    }
    let mut subsets = HashMap::new();
    for (lock, manifest) in manifests {
        // The subset root is its own workspace, not another manifest in the main workspace.
        let root_value = toml_value(&reader.read(manifest)?, manifest)?;
        if manifest == workspace_path
            || !root_value
                .get("workspace")
                .is_some_and(toml::Value::is_table)
        {
            return Err(format!(
                "subset_workspace_invalid: {manifest} は独立 workspace ではありません"
            ));
        }
        validate_subset_config(reader, manifest)?;
        let workspace = root_value
            .get("workspace")
            .and_then(toml::Value::as_table)
            .unwrap();
        if workspace
            .get("members")
            .is_some_and(|members| !members.as_array().is_some_and(Vec::is_empty))
        {
            return Err(format!(
                "subset_dependency_unsupported: {manifest}: 独立 workspace の追加 member は未対応です。単独 package の宣言へ直してください"
            ));
        }
        let mut expected = HashSet::new();
        subset_visit(
            reader,
            manifest,
            true,
            packages,
            &mut HashSet::new(),
            &mut expected,
        )?;
        if expected.is_empty() {
            return Err(format!(
                "subset_empty: {manifest} から本体 package へ到達しません"
            ));
        }
        let text = &files
            .iter()
            .find(|(path, _, _)| path == lock)
            .expect("declared lock")
            .1;
        validate_subset_lock(lock, text, packages, &expected)?;
        subsets.insert(lock.clone(), expected);
    }
    Ok(subsets)
}

// Cargo resolves configuration from the invocation directory and its ancestors,
// not from each path dependency's directory. Probe the same repo-local chain.
fn validate_subset_config(reader: &mut Reader, manifest: &str) -> Result<(), String> {
    for ancestor in Path::new(manifest).parent().unwrap().ancestors() {
        let directory = ancestor.join(".cargo").to_string_lossy().into_owned();
        if !reader.source.exists(&directory)? {
            continue;
        }
        reader.source.check_path_type(&directory, true)?;
        for name in ["config", "config.toml"] {
            let config = ancestor
                .join(".cargo")
                .join(name)
                .to_string_lossy()
                .into_owned();
            if reader.source.exists(&config)? {
                let value = toml_value(&reader.read(&config)?, &config)?;
                if ["source", "patch", "replace", "registries", "registry"]
                    .iter()
                    .any(|key| value.get(*key).is_some())
                {
                    return Err(format!(
                        "subset_dependency_unsupported: {config}: source・registry・patch/replace 設定は未対応です。標準 registry の宣言へ直すか設定の対応を確認してください"
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_subset_lock(
    lock: &str,
    text: &str,
    packages: &HashMap<String, String>,
    expected: &HashSet<String>,
) -> Result<(), String> {
    let lock_value = toml_value(text, lock)?;
    for package in lock_value
        .get("package")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(name) = package.get("name").and_then(toml::Value::as_str) {
            if package.get("source").is_none()
                && packages.values().any(|known| known == name)
                && !expected.contains(name)
            {
                return Err(format!(
                    "subset_unexpected_package: {lock}: {name} は静的閉包の外です。path 宣言と lock を確認してください"
                ));
            }
        }
        for dependency in package
            .get("dependencies")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
        {
            let dependency = dependency.as_str().ok_or_else(|| {
                format!(
                    "subset_dependency_unsupported: {lock}: dependencies は文字列にしてください"
                )
            })?;
            let mut parts = dependency.split_whitespace();
            if parts.next().is_some_and(|name| expected.contains(name)) && parts.next().is_some() {
                return Err(format!(
                    "subset_dependency_unsupported: {lock}: 版付き依存 {dependency} の書換えは未対応です。宣言と lock を確認してください"
                ));
            }
        }
    }
    Ok(())
}

fn subset_visit(
    reader: &mut Reader,
    manifest: &str,
    is_root: bool,
    packages: &HashMap<String, String>,
    visited: &mut HashSet<String>,
    expected: &mut HashSet<String>,
) -> Result<(), String> {
    if !visited.insert(manifest.to_owned()) {
        return Ok(());
    }
    if let Some(name) = packages.get(manifest) {
        expected.insert(name.clone());
    }
    let value = toml_value(&reader.read(manifest)?, manifest)?;
    let unsupported = |reason: &str| {
        format!(
            "subset_dependency_unsupported: {manifest}: {reason}。対応範囲の宣言へ直すか追加対応を確認してください"
        )
    };
    if ["target", "patch", "replace"]
        .iter()
        .any(|key| value.get(*key).is_some())
    {
        return Err(unsupported("target/patch/replace は未対応です"));
    }
    let mut paths = Vec::new();
    let mut registry = HashSet::new();
    for table in ["dependencies", "build-dependencies", "dev-dependencies"] {
        if table == "dev-dependencies" && !is_root {
            continue;
        }
        let Some(dependencies) = value.get(table) else {
            continue;
        };
        let dependencies = dependencies
            .as_table()
            .ok_or_else(|| unsupported("依存表が不正です"))?;
        for (name, dependency) in dependencies {
            if dependency.is_str() {
                registry.insert(name.as_str());
                continue;
            }
            let dependency = dependency
                .as_table()
                .ok_or_else(|| unsupported("依存の形が不明です"))?;
            if let Some(path) = dependency.get("path") {
                if dependency.keys().any(|key| key != "path") {
                    return Err(unsupported(
                        "条件・改名・workspace 継承等を持つ path 依存は未対応です",
                    ));
                }
                let path = path
                    .as_str()
                    .ok_or_else(|| unsupported("path は文字列で指定してください"))?;
                let next = reader.source.normalized_path(
                    &Path::new(manifest)
                        .parent()
                        .unwrap_or(Path::new(""))
                        .join(path)
                        .join("Cargo.toml")
                        .to_string_lossy(),
                )?;
                let target = toml_value(&reader.read(&next)?, &next)?;
                if target
                    .get("package")
                    .and_then(|package| package.get("name"))
                    .and_then(toml::Value::as_str)
                    != Some(name)
                {
                    return Err(unsupported("path 依存名と package.name が一致しません"));
                }
                paths.push(next);
            } else {
                if dependency.keys().any(|key| {
                    !matches!(
                        key.as_str(),
                        "version" | "optional" | "features" | "default-features"
                    )
                }) || !dependency.get("version").is_some_and(toml::Value::is_str)
                {
                    return Err(unsupported(
                        "registry 以外の source・依存の workspace 継承・不明な依存は未対応です",
                    ));
                }
                if ["optional", "default-features"]
                    .iter()
                    .any(|key| dependency.get(*key).is_some_and(|value| !value.is_bool()))
                    || dependency.get("features").is_some_and(|value| {
                        !value
                            .as_array()
                            .is_some_and(|features| features.iter().all(toml::Value::is_str))
                    })
                {
                    return Err(unsupported("registry 依存の属性の型が不正です"));
                }
                registry.insert(name.as_str());
            }
        }
    }
    if let Some(features) = value.get("features") {
        let features = features
            .as_table()
            .ok_or_else(|| unsupported("features が不正です"))?;
        for members in features.values() {
            for feature in members
                .as_array()
                .ok_or_else(|| unsupported("feature の形が不明です"))?
            {
                let feature = feature
                    .as_str()
                    .ok_or_else(|| unsupported("feature は文字列で指定してください"))?;
                let supported = if let Some(dependency) = feature.strip_prefix("dep:") {
                    registry.contains(dependency)
                } else if let Some((dependency, member)) = feature.split_once('/') {
                    registry.contains(dependency.strip_suffix('?').unwrap_or(dependency))
                        && !member.is_empty()
                        && !member.contains(['/', '?', ':'])
                } else {
                    registry.contains(feature) || features.contains_key(feature)
                };
                if !supported {
                    return Err(unsupported(
                        "path 依存への feature 伝播・未知の feature は未対応です",
                    ));
                }
            }
        }
    }
    for path in paths {
        subset_visit(reader, &path, false, packages, visited, expected)?;
    }
    Ok(())
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
        if !found.insert(name.to_owned()) {
            return Err(format!(
                "{path}: source 無し package {name} が重複しています"
            ));
        }
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
            Ok((_, declared)) => {
                let Some(mut release) = declared.release else {
                    return true;
                };
                match release.as_object_mut() {
                    Some(object) if !object.contains_key("versionFiles") => {
                        object.insert(
                            "versionFiles".to_owned(),
                            serde_json::Value::Array(Vec::new()),
                        );
                        serde_json::from_value::<Release>(release).is_ok()
                    }
                    _ => false,
                }
            }
            Err(_) => false,
        }
    }

    /// 版固定 file の path と、その `release` の欄を読む。
    fn declared_release(
        source: Source,
    ) -> Result<(&'static str, develop_tools_manifest::Declaration), String> {
        let (manifest, value) = Self::raw_declaration(source)?;
        let declared = develop_tools_manifest::declaration(&value)
            .map_err(|err| format!("{manifest}: {err}"))?;
        Ok((manifest, declared))
    }

    fn raw_declaration(source: Source) -> Result<(&'static str, serde_json::Value), String> {
        // 作業木か commit の tree か、同じ読み元の中で置き場を決める。作業木は共有の探索と同じ判定
        // （壊れた symlink も在るとみなす）にする。
        let located = match source {
            Source::Tree(root) => develop_tools_manifest::locate(root)?,
            Source::Commit { .. } => develop_tools_manifest::locate_in(|path| source.exists(path))?,
        };
        let manifest = located.unwrap_or(develop_tools_manifest::PATH);
        Ok((manifest, Self::read_declaration(source, manifest)?))
    }

    fn read_declaration(source: Source, manifest: &str) -> Result<serde_json::Value, String> {
        let raw = source.read(manifest)?;
        let value: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|err| format!("{manifest}: JSON が不正です: {err}"))?;
        Ok(value)
    }

    fn load_from(source: Source) -> Result<Self, String> {
        let (manifest, declared) = Self::declared_release(source)?;
        Self::from_declaration(source, manifest, declared)
    }

    fn from_declaration(
        source: Source,
        manifest: &str,
        declared: develop_tools_manifest::Declaration,
    ) -> Result<Self, String> {
        let release: Release = serde_json::from_value(declared.release.unwrap_or_default())
            .map_err(|err| format!("{manifest}: release が不正です: {err}"))?;
        if release.version_files.is_empty() {
            return Err("release.versionFiles が空です".to_owned());
        }
        let has_subset = release
            .version_files
            .iter()
            .any(|entry| entry.kind == Kind::CargoLockSubset);
        if has_subset && matches!(source, Source::Tree(_)) {
            source.regular_path(manifest)?;
        }
        let mut files = Self::from_source(source, release.version_files).map_err(|error| {
            if has_subset {
                let origin = match source { Source::Tree(_) => "作業木", Source::Commit { commit, .. } => commit };
                format!("subset_validation_failed: {origin}: {error}。同じ読み元の宣言・manifest・本体/独立 lock を確認してください")
            } else { error }
        })?;
        files.tag_prefix = declared.tag_prefix;
        files.changelog_stamp = declared.changelog_stamp;
        files.notes = declared
            .release_notes
            .as_ref()
            .map(crate::release_notes::normalize_notes)
            .transpose()?;
        Ok(files)
    }

    /// 版固定 file を読まず、引数の宣言から読む。CHANGELOG は刻印しない。
    pub fn from_entries(root: &Path, entries: Vec<Entry>) -> Result<Self, String> {
        Self::from_source(
            Source::Tree(root),
            entries.into_iter().map(DeclaredEntry::from).collect(),
        )
    }

    fn from_source(source: Source, entries: Vec<DeclaredEntry>) -> Result<Self, String> {
        if entries.is_empty() {
            return Err("版 file の宣言が空です".to_owned());
        }
        if entries
            .iter()
            .filter(|entry| entry.kind == Kind::CargoWorkspace)
            .count()
            > 1
        {
            return Err("cargo-workspace の宣言は1件だけにしてください".to_owned());
        }
        let mut manifests = HashMap::new();
        for entry in &entries {
            match (entry.kind, entry.manifest.as_deref()) {
                (Kind::CargoLockSubset, Some(manifest)) => {
                    let expected = Path::new(manifest).with_file_name("Cargo.lock");
                    if Path::new(manifest).file_name().and_then(|s| s.to_str())
                        != Some("Cargo.toml")
                        || manifest.is_empty()
                        || !tree_path(manifest).is_ok_and(|normalized| normalized == manifest)
                        || Path::new(manifest)
                            .components()
                            .any(|c| !matches!(c, Component::Normal(_)))
                        || expected != Path::new(&entry.path)
                    {
                        return Err(format!(
                            "subset_manifest_invalid: {} と manifest {manifest} の位置が対応しません",
                            entry.path
                        ));
                    }
                    manifests.insert(entry.path.clone(), manifest.to_owned());
                }
                (Kind::CargoLockSubset, None) => {
                    return Err(format!(
                        "subset_manifest_invalid: {} の manifest が必要です",
                        entry.path
                    ));
                }
                (_, Some(_)) => {
                    return Err(format!(
                        "subset_manifest_invalid: {} の kind に manifest は書けません",
                        entry.path
                    ));
                }
                (_, None) => {}
            }
        }
        let mut reader = Reader {
            source,
            regular: !manifests.is_empty(),
            texts: HashMap::new(),
        };
        let mut seen = HashSet::new();
        let entries = entries
            .into_iter()
            .map(|entry| {
                let relative = Path::new(&entry.path);
                if (!manifests.is_empty() && tree_path(&entry.path)? != entry.path)
                    || relative.as_os_str().is_empty()
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
                Ok(entry)
            })
            .collect::<Vec<Result<DeclaredEntry, String>>>();
        // Preserve the original read/diagnostic order: never probe beyond the
        // first invalid declaration, and defer its error until its turn to read.
        reader.prefetch(
            entries
                .iter()
                .map_while(|entry| entry.as_ref().ok())
                .map(|entry| entry.path.clone()),
        );
        let mut files = Vec::new();
        for entry in entries {
            let entry = entry?;
            let text = reader.read(&entry.path)?;
            files.push((entry.path.clone(), text, entry.kind));
        }
        if source.exists("package.json")?
            && !files
                .iter()
                .any(|(p, _, k)| p == "package.json" && *k == Kind::PackageJson)
        {
            return Err("root の package.json が版 file に宣言されていません".to_owned());
        }
        let packages = lock_packages(&mut reader, &files)?;
        let names = &packages.names;
        let subsets = if !manifests.is_empty() {
            subset_targets(&mut reader, &files, &manifests, &packages.workspace)?
        } else {
            HashMap::new()
        };
        let versions = files
            .iter()
            .map(|(path, text, kind)| match kind {
                Kind::PackageJson => package_version(text, path),
                Kind::CargoLock => lock_version(text, path, names),
                Kind::CargoLockSubset => lock_version(text, path, &subsets[path]),
                _ => version_at(
                    &toml_value(text, path)?,
                    kind.toml_location().unwrap().0,
                    path,
                ),
            })
            .collect::<Result<Vec<_>, String>>()?;
        let origin = files
            .iter()
            .position(|(_, _, kind)| *kind == Kind::CargoWorkspace)
            .unwrap_or(0);
        let current = versions[origin];
        let origin_path = &files[origin].0;
        for ((path, _, _), version) in files.iter().zip(versions) {
            if version != current {
                return Err(format!(
                    "{path} の版 ({version}) が {origin_path} の版 ({current}) と一致しません"
                ));
            }
        }
        Ok(Self {
            files,
            current,
            cargo_names: packages.names,
            subsets,
            tag_prefix: TagPrefix::default(),
            changelog_stamp: false,
            notes: None,
        })
    }

    pub fn current(&self) -> Version {
        self.current
    }

    pub fn tag(&self, version: Version) -> String {
        self.tag_prefix.format(version)
    }

    pub fn parse_tag(&self, tag: &str) -> Option<Version> {
        self.tag_prefix.parse(tag)
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
        let names = &self.cargo_names;
        let mut changes = Vec::<(PathBuf, String)>::new();
        for (relative, text, kind) in &self.files {
            let updated = match kind {
                Kind::PackageJson => replace_json_version(text, next)?,
                Kind::CargoLock => replace_lock_versions(text, names, self.current, next)?,
                Kind::CargoLockSubset => {
                    replace_lock_versions(text, &self.subsets[relative], self.current, next)?
                }
                _ => replace_toml_version(text, kind.toml_location().unwrap().1, next)?,
            };
            changes.push((root.join(relative), updated));
        }
        let mut written: Vec<String> = self.files.iter().map(|(path, _, _)| path.clone()).collect();
        // 宣言の無い repo では CHANGELOG を読まない。宣言があって file が無いのは宣言の誤りなので止める。
        if self.changelog_stamp {
            let repo = crate::release_notes::remote_repo(root)?;
            let (stamped, count) = crate::stamp_changelog_with_prefix(
                &read(root, CHANGELOG)?,
                next,
                &repo,
                self.notes,
                &self.tag_prefix,
            )?;
            if count > 0 {
                changes.push((root.join(CHANGELOG), stamped));
                written.push(CHANGELOG.to_owned());
            }
        }
        for lock in dependent_locks {
            let text = std::fs::read_to_string(lock)
                .map_err(|err| format!("{} を読めません: {err}", lock.display()))?;
            let updated = replace_dependent_lock_versions(&text, names, self.current, next)
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
            let Some(tagged) = self.parse_tag(tag) else {
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

    struct ChildFixture(std::process::Child);

    impl Drop for ChildFixture {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    struct StdoutReadFailure {
        _pipe: std::process::ChildStdout,
    }

    impl std::io::Read for StdoutReadFailure {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("injected stdout read failure"))
        }
    }

    #[test]
    fn stdout_read_error_reaps_batch_child_before_returning() {
        use std::process::{Command, Stdio};
        let mut child = ChildFixture(
            Command::new("/bin/sh")
                .args(["-c", "exec sleep 0.05"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let pid = child.0.id().to_string();
        let stdout = StdoutReadFailure {
            _pipe: child.0.stdout.take().unwrap(),
        };
        let result = batch_output(&mut child.0, "x".repeat(128 * 1024), stdout);
        assert!(result.is_none());
        // kill -0 also sees a zombie, so this checks wait/reaping before Drop.
        let present = Command::new("/bin/sh")
            .args(["-c", "kill -0 \"$1\" 2>/dev/null", "fixture", &pid])
            .status()
            .unwrap();
        assert!(
            !present.success(),
            "batch child {pid} must be reaped before returning an I/O error"
        );
    }

    fn version(major: u64, minor: u64, patch: u64) -> Version {
        Version {
            major,
            minor,
            patch,
        }
    }

    fn declare(root: &Path, entries: &str) {
        std::fs::create_dir_all(root.join(".config")).unwrap();
        std::fs::write(
            root.join(".config/develop-tools.json"),
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
    fn multiple_workspace_sources_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let manifest = "[workspace]\nmembers = []\n[workspace.package]\nversion = \"1.2.3\"\n";
        std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
        std::fs::write(root.join("other.toml"), manifest).unwrap();
        declare(
            root,
            r#"{"path":"Cargo.toml","kind":"cargo-workspace"},{"path":"other.toml","kind":"cargo-workspace"}"#,
        );
        let error = VersionFiles::load(root)
            .err()
            .expect("multiple workspace sources must fail");
        assert!(error.contains("cargo-workspace"), "{error}");
    }

    #[test]
    fn workspace_is_the_version_source_regardless_of_entry_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers=[]\n[workspace.package]\nversion=\"1.2.3\"\n",
        )
        .unwrap();
        std::fs::write(root.join("package.json"), "{\"version\":\"9.0.0\"}").unwrap();
        let cargo = r#"{"path":"Cargo.toml","kind":"cargo-workspace"}"#;
        let package = r#"{"path":"package.json","kind":"package-json"}"#;
        for entries in [format!("{package},{cargo}"), format!("{cargo},{package}")] {
            declare(root, &entries);
            let error = VersionFiles::load(root).err().unwrap();
            assert_eq!(
                error,
                "package.json の版 (9.0.0) が Cargo.toml の版 (1.2.3) と一致しません"
            );
        }
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
        std::fs::create_dir_all(root.join(".config")).unwrap();
        std::fs::write(
            root.join(".config/develop-tools.json"),
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
        let legacy = VersionFiles::load(root).err().unwrap();
        assert!(legacy.starts_with("manifest_legacy:"), "{legacy}");
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
