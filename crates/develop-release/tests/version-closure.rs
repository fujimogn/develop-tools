use develop_release::{Version, version_files::VersionFiles};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::Builder::new()
            .prefix("develop-meta-version-closure-")
            .tempdir()
            .unwrap();
        let fixture = Self {
            root: temp.path().join("repo"),
            _temp: temp,
        };
        fixture.write("main/Cargo.toml", "[workspace]\nresolver='2'\nmembers=['engine','board','unused']\n[workspace.package]\nversion='0.1.0'\nedition='2021'\n");
        for (name, deps) in [
            (
                "engine",
                "[dependencies]\nboard={path='../board'}\n[dev-dependencies]\nunused={path='../unused'}\n",
            ),
            ("board", ""),
            ("unused", ""),
        ] {
            fixture.write(&format!("main/{name}/Cargo.toml"), &format!("[package]\nname='{name}'\nversion.workspace=true\nedition.workspace=true\n{deps}"));
            fixture.write(&format!("main/{name}/src/lib.rs"), "pub fn fixture() {}\n");
        }
        fixture.write("main/Cargo.lock", "version = 3\n\n[[package]]\nname = \"engine\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"board\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"unused\"\nversion = \"0.1.0\"\n");
        fixture.write("learner/Cargo.toml", "[workspace]\nresolver='2'\n[package]\nname='learner'\nversion='9.0.0'\nedition='2021'\n[dependencies]\nengine={path='../main/engine'}\n");
        fixture.write("learner/src/lib.rs", "pub fn learner() {}\n");
        fixture.write("learner/Cargo.lock", "version = 3\n\n[[package]]\nname = \"learner\"\nversion = \"9.0.0\"\n\n[[package]]\nname = \"engine\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"board\"\nversion = \"0.1.0\"\n");
        fixture.write(".config/develop-tools.json", r#"{"integrationBranch":"main","release":{"versionFiles":[{"path":"main/Cargo.toml","kind":"cargo-workspace"},{"path":"main/Cargo.lock","kind":"cargo-lock"},{"path":"learner/Cargo.lock","kind":"cargo-lock-subset","manifest":"learner/Cargo.toml"}]}}"#);
        fixture.git(&["init", "-q", "-b", "main"]);
        fixture.git(&["config", "user.name", "Fixture"]);
        fixture.git(&["config", "user.email", "fixture@example.invalid"]);
        fixture.git(&["config", "commit.gpgsign", "false"]);
        fixture.seal();
        fixture
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.root.join(relative)).unwrap()
    }

    fn git(&self, args: &[&str]) -> String {
        let output = develop_tools_core::git(&self.root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn seal(&self) {
        self.git(&["add", "."]);
        self.git(&["commit", "--allow-empty", "-qm", "fixture"]);
    }

    fn snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        self.git(&["ls-files"])
            .lines()
            .map(|path| {
                (
                    path.to_owned(),
                    std::fs::read(self.root.join(path)).unwrap(),
                )
            })
            .collect()
    }

    fn assert_unsupported(&self) {
        self.seal();
        let before = self.snapshot();
        let refs = self.git(&["show-ref"]);
        let commit = self.git(&["rev-parse", "HEAD"]);
        for result in [
            VersionFiles::load(&self.root),
            VersionFiles::load_at(&self.root, &commit),
        ] {
            let error = result.err().expect("unsupported closure must fail closed");
            assert!(error.contains("subset_dependency_unsupported"), "{error}");
            assert!(
                error.contains("確認") || error.contains("直す"),
                "next action missing: {error}"
            );
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_check-release-version"));
        develop_tools_core::strip_git_env(&mut command);
        let output = command.current_dir(&self.root).output().unwrap();
        assert!(
            !output.status.success(),
            "unsupported declaration must stop the CLI"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("subset_dependency_unsupported"));
        assert_eq!(self.snapshot(), before);
        assert_eq!(self.git(&["show-ref"]), refs);
    }
}

#[test]
fn path_version_constraints_are_not_accepted_without_a_manifest_update_contract() {
    let fixture = Fixture::new();
    fixture.write(
        "learner/Cargo.toml",
        &fixture.read("learner/Cargo.toml").replace(
            "path='../main/engine'",
            "path='../main/engine',version='^0.1'",
        ),
    );
    fixture.assert_unsupported();
}

#[test]
fn nested_source_replacement_is_not_ignored() {
    let fixture = Fixture::new();
    fixture.write(
        "learner/.cargo/config.toml",
        "[source.crates-io]\nreplace-with='mirror'\n[source.mirror]\ndirectory='vendor'\n",
    );
    fixture.assert_unsupported();
}

#[test]
fn malformed_registry_attributes_are_not_accepted_as_leaves() {
    let fixture = Fixture::new();
    fixture.write(
        "learner/Cargo.toml",
        &(fixture.read("learner/Cargo.toml") + "serde={version='1',optional=42}\n"),
    );
    fixture.assert_unsupported();
}

macro_rules! unsupported_manifest {
    ($name:ident, $text:expr) => {
        #[test]
        fn $name() {
            let fixture = Fixture::new();
            fixture.write("learner/Cargo.toml", $text);
            fixture.assert_unsupported();
        }
    };
}
const ROOT: &str =
    "[workspace]\nresolver='2'\n[package]\nname='learner'\nversion='9.0.0'\nedition='2021'\n";
macro_rules! unsupported_dependency {
    ($name:ident, $dependency:expr) => {
        unsupported_manifest!(
            $name,
            &format!("{ROOT}[dependencies]\nengine={}\n", $dependency)
        );
    };
}
unsupported_dependency!(path_feature, "{path='../main/engine',features=['gpu']}");
unsupported_dependency!(path_optional, "{path='../main/engine',optional=true}");
unsupported_dependency!(
    path_default_features,
    "{path='../main/engine',default-features=false}"
);
unsupported_dependency!(dependency_workspace, "{workspace=true}");
unsupported_dependency!(
    dependency_rename,
    "{path='../main/engine',package='engine'}"
);
unsupported_dependency!(git_source, "{git='https://example.invalid/engine'}");
unsupported_dependency!(alternate_registry, "{version='1',registry='other'}");
unsupported_dependency!(unknown_dependency, "42");
unsupported_dependency!(unknown_dependency_field, "{version='1',mystery=true}");
unsupported_manifest!(
    target_table,
    &format!(
        "{ROOT}[dependencies]\nengine={{path='../main/engine'}}\n[target.'cfg(unix)'.dependencies]\nboard={{path='../main/board'}}\n"
    )
);
unsupported_manifest!(
    patch_table,
    &format!(
        "{ROOT}[dependencies]\nengine={{path='../main/engine'}}\n[patch.crates-io]\nserde={{path='../main/board'}}\n"
    )
);
unsupported_manifest!(
    replace_table,
    &format!(
        "{ROOT}[dependencies]\nengine={{path='../main/engine'}}\n[replace]\n'serde:1.0.0'={{path='../main/board'}}\n"
    )
);
unsupported_manifest!(
    path_feature_selection,
    &format!(
        "{ROOT}[dependencies]\nengine={{path='../main/engine'}}\n[features]\ngpu=['engine/gpu']\n"
    )
);
unsupported_manifest!(
    unknown_feature,
    &format!(
        "{ROOT}[dependencies]\nengine={{path='../main/engine'}}\n[features]\ngpu=['unknown']\n"
    )
);

#[test]
fn workspace_extra_members() {
    let fixture = Fixture::new();
    fixture.write(
        "learner/Cargo.toml",
        &fixture
            .read("learner/Cargo.toml")
            .replace("[workspace]", "[workspace]\nmembers=['extra']"),
    );
    fixture.write(
        "learner/extra/Cargo.toml",
        "[package]\nname='extra'\nversion='0.1.0'\n",
    );
    fixture.write("learner/extra/src/lib.rs", "");
    fixture.assert_unsupported();
}

// Cargo is an offline fixture oracle only. Production never invokes it.
fn cargo_output(directory: &Path, home: &Path, args: &[&str]) -> serde_json::Value {
    let mut command = Command::new(env!("CARGO"));
    command
        .current_dir(directory)
        .env("CARGO_HOME", home)
        .args(args);
    develop_tools_core::strip_git_env(&mut command);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "offline oracle {args:?} must not skip: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    if args[0] == "metadata" {
        serde_json::from_slice(&output.stdout).unwrap()
    } else {
        serde_json::Value::Null
    }
}

fn oracle_reachable(metadata: &serde_json::Value) -> BTreeSet<String> {
    let resolve = &metadata["resolve"];
    let root = resolve["root"].as_str().unwrap();
    let nodes: BTreeMap<_, _> = resolve["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| (node["id"].as_str().unwrap(), node))
        .collect();
    let mut reached = BTreeSet::new();
    let mut todo = vec![root];
    while let Some(id) = todo.pop() {
        if !reached.insert(id.to_owned()) {
            continue;
        }
        for dependency in nodes[id]["deps"].as_array().unwrap() {
            if id == root
                || dependency["dep_kinds"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|kind| kind["kind"].as_str() != Some("dev"))
            {
                todo.push(dependency["pkg"].as_str().unwrap());
            }
        }
    }
    reached
}

fn lock_packages(text: &str) -> BTreeMap<String, String> {
    let value: toml::Value = toml::from_str(text).unwrap();
    value["package"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p.get("source").is_none())
        .map(|p| {
            (
                p["name"].as_str().unwrap().to_owned(),
                p["version"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn gpu_registry_feature_static_closure_matches_locked_offline_metadata() {
    assert_metadata_closure(false);
}

#[test]
fn root_build_and_dev_dependencies_match_locked_offline_metadata() {
    assert_metadata_closure(true);
}

fn assert_metadata_closure(root_dev: bool) {
    let fixture = Fixture::new();
    fixture.write(
        "main/Cargo.toml",
        &fixture
            .read("main/Cargo.toml")
            .replace("version='0.1.0'", "version=\"0.1.0\""),
    );
    if root_dev {
        fixture.write("learner/Cargo.toml", &(fixture.read("learner/Cargo.toml") + "[build-dependencies]\nboard={path='../main/board'}\n[dev-dependencies]\nunused={path='../main/unused'}\n"));
    }
    // The production shape also has inherited non-root dev dependencies.
    fixture.write(
        "main/Cargo.toml",
        &(fixture.read("main/Cargo.toml") + "[workspace.dependencies]\nunused={path='unused'}\n"),
    );
    fixture.write(
        "main/engine/Cargo.toml",
        &fixture
            .read("main/engine/Cargo.toml")
            .replace("unused={path='../unused'}", "unused={workspace=true}"),
    );
    // serde is already required to build this test. Pin the same checked-in
    // version and reuse only the registry cache, never user Cargo configuration.
    let cli_lock = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    let lock: toml::Value = toml::from_str(&std::fs::read_to_string(cli_lock).unwrap()).unwrap();
    let serde_version = lock["package"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"].as_str() == Some("serde"))
        .unwrap()["version"]
        .as_str()
        .unwrap();
    fixture.write(
        "learner/Cargo.toml",
        &(fixture.read("learner/Cargo.toml").replace(
            "[dependencies]",
            &format!("[dependencies]\nserde={{version='={serde_version}',optional=true}}"),
        ) + "[features]\ncuda=['dep:serde']\n"),
    );
    let host_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".cargo"));
    let cargo_home = fixture._temp.path().join("cargo-home");
    std::fs::create_dir(&cargo_home).unwrap();
    // Only the registry is shared, including any index/cache writes. This
    // isolated CARGO_HOME has its own cache lock, so it does not serialize other
    // Cargo processes. An already populated offline cache is a fixture premise.
    std::os::unix::fs::symlink(host_home.join("registry"), cargo_home.join("registry")).unwrap();
    // Fixture preparation may resolve its own lock, offline. All comparisons
    // below require --locked, and no production path repairs a lock.
    cargo_output(
        &fixture.root.join("learner"),
        &cargo_home,
        &["generate-lockfile", "--offline"],
    );
    cargo_output(
        &fixture.root.join("main"),
        &cargo_home,
        &["generate-lockfile", "--offline"],
    );
    fixture.seal();
    let before = fixture.snapshot();
    let expected: BTreeSet<_> = if root_dev {
        vec!["engine", "board", "unused"]
    } else {
        vec!["engine", "board"]
    }
    .into_iter()
    .map(str::to_owned)
    .collect();
    let main_manifests: BTreeMap<_, _> = ["engine", "board", "unused"]
        .into_iter()
        .map(|name| {
            (
                fixture
                    .root
                    .join(format!("main/{name}/Cargo.toml"))
                    .canonicalize()
                    .unwrap(),
                name.to_owned(),
            )
        })
        .collect();
    for enabled in [false, true] {
        let mut args = vec!["metadata", "--format-version=1", "--locked", "--offline"];
        if enabled {
            args.extend(["--features", "cuda"]);
        }
        let metadata = cargo_output(&fixture.root.join("learner"), &cargo_home, &args);
        let reachable = oracle_reachable(&metadata);
        let reached_packages: Vec<_> = metadata["packages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| reachable.contains(p["id"].as_str().unwrap()))
            .collect();
        assert_eq!(
            reached_packages.iter().any(|p| p["name"] == "serde"),
            enabled
        );
        let oracle: BTreeSet<_> = reached_packages
            .iter()
            .filter_map(|p| {
                main_manifests
                    .get(&PathBuf::from(p["manifest_path"].as_str().unwrap()))
                    .cloned()
            })
            .collect();
        assert_eq!(oracle, expected, "feature cuda={enabled}");
    }
    assert_eq!(
        fixture.snapshot(),
        before,
        "locked oracle must not alter fixture files"
    );
    let subset_before = fixture.read("learner/Cargo.lock");
    let next: Version = "0.1.1".parse().unwrap();
    let files = VersionFiles::load(&fixture.root).unwrap();
    files.write(&fixture.root, next).unwrap();
    let after = fixture.read("learner/Cargo.lock");
    let observed: BTreeSet<_> = lock_packages(&after)
        .into_iter()
        .filter_map(|(name, version)| (version == "0.1.1").then_some(name))
        .collect();
    assert_eq!(
        observed, expected,
        "public write observes the static closure"
    );
    assert_eq!(lock_packages(&after)["learner"], "9.0.0");
    let mut expected_bytes = subset_before;
    for name in expected {
        expected_bytes = expected_bytes.replace(
            &format!("name = \"{name}\"\nversion = \"0.1.0\""),
            &format!("name = \"{name}\"\nversion = \"0.1.1\""),
        );
    }
    assert_eq!(
        after, expected_bytes,
        "registry and non-target bytes remain unchanged"
    );
    assert_eq!(
        fixture.read("learner/Cargo.toml").as_bytes(),
        &before["learner/Cargo.toml"]
    );
}

unsupported_manifest!(
    malformed_feature_token,
    &format!(
        "{ROOT}[dependencies]\nengine={{path='../main/engine'}}\nserde={{version='1',optional=true}}\n[features]\ngpu=['dep:serde/one/two']\n"
    )
);

#[test]
fn configuration_sources_are_rejected_at_every_invocation_ancestor() {
    for directory in [".cargo", "learner/.cargo"] {
        for name in ["config", "config.toml"] {
            for setting in [
                "[source.crates-io]\nreplace-with='mirror'\n",
                "[patch.crates-io]\nserde={path='vendor'}\n",
                "[registries.other]\nindex='https://example.invalid'\n",
            ] {
                let fixture = Fixture::new();
                fixture.write(&format!("{directory}/{name}"), setting);
                fixture.assert_unsupported();
            }
        }
    }
}

unsupported_manifest!(
    path_name_mismatch,
    &format!("{ROOT}[dependencies]\nalias={{path='../main/engine'}}\n")
);

#[test]
fn registry_feature_and_default_flag_types_must_be_known() {
    for dependency in [
        "{version='1',features=false}",
        "{version='1',default-features=42}",
    ] {
        let fixture = Fixture::new();
        fixture.write(
            "learner/Cargo.toml",
            &(fixture.read("learner/Cargo.toml") + &format!("serde={dependency}\n")),
        );
        fixture.assert_unsupported();
    }
}
