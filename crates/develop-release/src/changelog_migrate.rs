use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
};

use develop_tools_core::changelog_title;
use serde::{Deserialize, Serialize};
use serde_json::json;

type Result<T> = std::result::Result<T, String>;

#[derive(Clone)]
struct Options {
    repo: PathBuf,
    check: bool,
    write: bool,
    json: bool,
    plan_hash: Option<String>,
    resolutions: Option<PathBuf>,
}

#[derive(Clone)]
struct Item {
    line: usize,
    key: String,
    date_end: usize,
    suffix_end: usize,
    legacy: Option<String>,
    legacy_segment: Option<String>,
    stamped: bool,
    supplement_versions: Vec<String>,
    existing_release_url: Option<String>,
}

#[derive(Clone, Default)]
struct Origin {
    commit: String,
    ambiguous: bool,
    reason: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanItem {
    line: usize,
    key: String,
    introduction: Option<String>,
    stamp: Option<String>,
    proposed_stamp: String,
    release_url: Option<String>,
    reason: Option<String>,
    resolution_reason: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    head: String,
    source_blob: String,
    plan_hash: String,
    origin: String,
    items: Vec<PlanItem>,
    holds: usize,
    invalid: usize,
    invalid_lines: Vec<usize>,
    source_bytes: usize,
    target_bytes: usize,
    release_fetched_at: String,
    #[serde(skip_serializing)]
    available_tags: HashSet<String>,
    #[serde(skip_serializing)]
    releases: HashSet<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolutionFile {
    source_blob: String,
    plan_hash: String,
    items: Vec<Resolution>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Resolution {
    key: String,
    line: usize,
    stamp: String,
    reason: String,
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = develop_tools_core::git(repo)
        .args(args)
        .output()
        .map_err(|error| format!("git {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim_end_matches('\n').to_owned())
        .map_err(|error| error.to_string())
}

fn parse_options(args: Vec<String>) -> Result<Options> {
    let mut options = Options {
        repo: PathBuf::new(),
        check: false,
        write: false,
        json: false,
        plan_hash: None,
        resolutions: None,
    };
    for arg in args {
        if let Some(value) = arg.strip_prefix("--repo=") {
            options.repo = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--plan-hash=") {
            options.plan_hash = Some(value.to_owned());
        } else if let Some(value) = arg.strip_prefix("--resolutions=") {
            options.resolutions = Some(PathBuf::from(value));
        } else {
            match arg.as_str() {
                "--check" => options.check = true,
                "--write" => options.write = true,
                "--json" => options.json = true,
                _ => return Err(format!("unknown argument: {arg}")),
            }
        }
    }
    if !options.repo.is_absolute() {
        return Err("--repo must be an absolute path".into());
    }
    if options.write && options.plan_hash.is_none() {
        return Err("--write requires --plan-hash".into());
    }
    if options.write && options.check {
        return Err("--check and --write cannot be combined".into());
    }
    Ok(options)
}

fn origin(repo: &Path) -> Result<String> {
    let url = git(repo, &["config", "--get", "remote.origin.url"])?;
    let tail = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))
        .ok_or_else(|| "origin is not a GitHub URL".to_owned())?;
    let identity = tail
        .strip_suffix(".git")
        .unwrap_or(tail)
        .trim_end_matches('/');
    let (owner, name) = identity
        .split_once('/')
        .ok_or_else(|| "origin has no owner/repo".to_owned())?;
    let valid_name = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    if !valid_name(owner) || !valid_name(name) {
        return Err("origin has no unique owner/repo".into());
    }
    Ok(format!("{owner}/{name}"))
}

fn validate_repo(repo: &Path) -> Result<()> {
    if git(repo, &["rev-parse", "--is-shallow-repository"])? == "true" {
        return Err("shallow history".into());
    }
    if !git(repo, &["status", "--porcelain", "--", "CHANGELOG.md"])?.is_empty() {
        return Err("dirty CHANGELOG.md".into());
    }
    let manifest = fs::read(repo.join(".config/develop-tools.json"))
        .map_err(|error| format!("release declaration unavailable: {error}"))?;
    let value: serde_json::Value = serde_json::from_slice(&manifest)
        .map_err(|error| format!("release declaration invalid: {error}"))?;
    let versions = value["release"]["versionFiles"]
        .as_array()
        .ok_or("release.versionFiles is unavailable")?;
    if versions.is_empty() {
        return Err("repo has no release.versionFiles".into());
    }
    if value["release"]["notes"]["fallback"] == "changelog-section" {
        return Err(
            "changelog-section uses version sections and is outside migration scope".into(),
        );
    }
    Ok(())
}

fn parse_item(line: &str, number: usize) -> Option<Item> {
    if let Some(parsed) = changelog_title::parse(line) {
        return Some(Item {
            line: number,
            key: format!("{}\u{1f}{}", parsed.title, parsed.date),
            date_end: parsed.stamp_range.start,
            suffix_end: parsed.metadata_range.end - '）'.len_utf8(),
            legacy: None,
            legacy_segment: None,
            stamped: true,
            supplement_versions: Vec::new(),
            existing_release_url: parsed.release_url.map(str::to_owned),
        });
    }
    let text = line.strip_prefix("- **")?;
    let title_end = changelog_title::title_closing_marker(text)?;
    let title = &text[..title_end];
    let summary = text[title_end + 2..].strip_prefix(':')?;
    if title.trim().is_empty() || summary.trim().is_empty() {
        return None;
    }
    let date_open = title.rfind('（')?;
    if title[..date_open].trim().is_empty() {
        return None;
    }
    let date_close = title[date_open..].find('）')? + date_open;
    if date_close + '）'.len_utf8() != title.len() {
        return None;
    }
    let metadata = &title[date_open + '（'.len_utf8()..date_close];
    let date = metadata.get(..10)?;
    if !develop_tools_core::changelog_title::valid_date(date) {
        return None;
    }
    let date_end = 4 + date_open + '（'.len_utf8() + 10;
    let suffix_end = 4 + date_close;
    let tail = &metadata[10..];
    if !tail.is_empty() && !tail.starts_with('、') {
        return None;
    }
    if tail.ends_with('、') {
        return None;
    }
    let legacy_matches: Vec<_> = tail
        .split('、')
        .filter_map(|part| {
            if part == "未リリース" {
                Some(("pending".to_owned(), format!("、{part}")))
            } else {
                part.strip_suffix(" で prod 反映")
                    .filter(|version| version_tag(version))
                    .map(|version| (version.to_owned(), format!("、{part}")))
            }
        })
        .collect();
    if legacy_matches.len() > 1 {
        return None;
    }
    let (legacy, legacy_segment) = legacy_matches
        .into_iter()
        .next()
        .map_or((None, None), |(stamp, segment)| {
            (Some(stamp), Some(segment))
        });
    Some(Item {
        line: number,
        key: format!("{}\u{1f}{}", &title[..date_open], date),
        date_end,
        suffix_end,
        legacy,
        legacy_segment,
        stamped: false,
        supplement_versions: versions_in_supplement(tail),
        existing_release_url: None,
    })
}

fn parse_historical_item(line: &str, number: usize) -> Option<Item> {
    parse_item(line, number).or_else(|| {
        let (date, _) = changelog_title::parse_legacy_compact(line)?;
        let rest = line.strip_prefix("- **")?;
        let title = &rest[..changelog_title::title_closing_marker(rest)?];
        Some(Item {
            line: number,
            key: format!("{title}\u{1f}{date}"),
            date_end: 0,
            suffix_end: 0,
            legacy: None,
            legacy_segment: None,
            stamped: false,
            supplement_versions: Vec::new(),
            existing_release_url: None,
        })
    })
}

fn items(bytes: &[u8]) -> Result<Vec<Item>> {
    let (items, invalid) = scan_items(bytes)?;
    if let Some(line) = invalid.first() {
        return Err(format!("invalid CHANGELOG item at line {line}"));
    }
    Ok(items)
}

fn scan_items(bytes: &[u8]) -> Result<(Vec<Item>, Vec<usize>)> {
    let text =
        std::str::from_utf8(bytes).map_err(|error| format!("CHANGELOG is not UTF-8: {error}"))?;
    let mut result = Vec::new();
    let mut invalid = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.starts_with("- **") {
            if let Some(item) = parse_item(line, index + 1) {
                result.push(item);
            } else {
                invalid.push(index + 1);
            }
        }
    }
    Ok((result, invalid))
}

fn historical_items(bytes: &[u8]) -> Result<(Vec<Item>, HashSet<String>)> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("historical CHANGELOG is not UTF-8: {error}"))?;
    let mut parsed = Vec::new();
    let mut unknown_titles = HashSet::new();
    for (index, line) in text.lines().enumerate() {
        if !line.starts_with("- **") {
            continue;
        }
        if let Some(item) = parse_historical_item(line, index + 1) {
            parsed.push(item);
        } else {
            let title = line
                .strip_prefix("- **")
                .and_then(|rest| rest.split_once("**"))
                .map(|(title, _)| title.trim())
                .filter(|title| !title.is_empty())
                .ok_or_else(|| format!("unidentifiable historical item at line {}", index + 1))?;
            unknown_titles.insert(title.to_owned());
            for (offset, _) in title.match_indices(['（', '(']) {
                let normalized = title[..offset].trim_end();
                if !normalized.is_empty() {
                    unknown_titles.insert(normalized.to_owned());
                }
            }
        }
    }
    Ok((parsed, unknown_titles))
}

fn versions_in_supplement(tail: &str) -> Vec<String> {
    let bytes = tail.as_bytes();
    let mut versions = Vec::new();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'v' || index > 0 && bytes[index - 1].is_ascii_alphanumeric() {
            continue;
        }
        let rest = &tail[index..];
        let end = rest
            .char_indices()
            .skip(1)
            .find(|(_, character)| !character.is_ascii_digit() && *character != '.')
            .map_or(rest.len(), |(offset, _)| offset);
        let candidate = &rest[..end];
        if version_tag(candidate) {
            versions.push(candidate.to_owned());
        }
    }
    versions
}

fn version_tag(value: &str) -> bool {
    value.strip_prefix('v').is_some_and(|rest| {
        let parts: Vec<_> = rest.split('.').collect();
        parts.len() == 3
            && parts.iter().all(|part| {
                !part.is_empty()
                    && (part.len() == 1 || !part.starts_with('0'))
                    && part.bytes().all(|byte| byte.is_ascii_digit())
            })
    })
}

fn validate_stamped(items: &[Item], identity: &str) -> Result<()> {
    for item in items {
        if let Some(url) = &item.existing_release_url {
            if !url.starts_with(&format!("https://github.com/{identity}/releases/tag/")) {
                return Err(format!("stamp URL origin mismatch at line {}", item.line));
            }
        }
    }
    Ok(())
}

fn historical_blob(repo: &Path, commit: &str) -> Result<Vec<u8>> {
    if git(
        repo,
        &["ls-tree", "--name-only", commit, "--", "CHANGELOG.md"],
    )?
    .is_empty()
    {
        return Ok(Vec::new());
    }
    let spec = format!("{commit}:CHANGELOG.md");
    let output = develop_tools_core::git(repo)
        .args(["show", &spec])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "git show {spec}: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(output.stdout)
}

fn replacement_keys(repo: &Path, commit: &str, parents: &[&str]) -> Result<HashSet<String>> {
    let mut replacements = HashSet::new();
    for parent in parents {
        let diff = git(
            repo,
            &[
                "diff",
                "--no-ext-diff",
                "--no-color",
                "--unified=0",
                parent,
                commit,
                "--",
                "CHANGELOG.md",
            ],
        )?;
        let mut removed = HashSet::new();
        let mut added = HashSet::new();
        let flush = |removed: &mut HashSet<String>,
                     added: &mut HashSet<String>,
                     replacements: &mut HashSet<String>| {
            if !removed.is_empty() {
                replacements.extend(added.drain().filter(|key| !removed.contains(key)));
            }
            removed.clear();
            added.clear();
        };
        for line in diff.lines() {
            if line.starts_with("@@") {
                flush(&mut removed, &mut added, &mut replacements);
            } else if let Some(content) = line.strip_prefix('-') {
                if !line.starts_with("---") {
                    if let Some(item) = parse_historical_item(content, 0) {
                        removed.insert(item.key);
                    } else if content.starts_with("- **") {
                        removed.insert("\0unparsed".to_owned());
                    }
                }
            } else if let Some(content) = line.strip_prefix('+') {
                if !line.starts_with("+++") {
                    if let Some(item) = parse_historical_item(content, 0) {
                        added.insert(item.key);
                    }
                }
            }
        }
        flush(&mut removed, &mut added, &mut replacements);
    }
    Ok(replacements)
}

fn changelog_renamed(repo: &Path, commit: &str, parents: &[&str]) -> Result<bool> {
    for parent in parents {
        let changes = git(
            repo,
            &[
                "diff",
                "--no-ext-diff",
                "--no-color",
                "--find-renames",
                "--find-copies",
                "--name-status",
                parent,
                commit,
            ],
        )?;
        if changes.lines().any(|line| {
            let mut fields = line.split('\t');
            fields
                .next()
                .is_some_and(|status| status.starts_with('R') || status.starts_with('C'))
                && fields.next().is_some()
                && fields.next() == Some("CHANGELOG.md")
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn history(repo: &Path, head: &str, current: &[Item]) -> Result<HashMap<String, Origin>> {
    let graph = git(
        repo,
        &["rev-list", "--reverse", "--topo-order", "--parents", head],
    )?;
    let changed: HashSet<String> = git(
        repo,
        &[
            "log",
            "--full-history",
            "-m",
            "--format=%H",
            head,
            "--",
            "CHANGELOG.md",
        ],
    )?
    .lines()
    .map(str::to_owned)
    .collect();
    let mut states: HashMap<String, Rc<BTreeMap<String, Origin>>> = HashMap::new();
    let mut unknown_states: HashMap<String, Rc<HashSet<String>>> = HashMap::new();
    for row in graph.lines() {
        let mut parts = row.split_whitespace();
        let commit = parts.next().ok_or("empty revision row")?;
        let parents: Vec<_> = parts.collect();
        let parent_states: Vec<_> = parents
            .iter()
            .map(|parent| {
                states
                    .get(*parent)
                    .cloned()
                    .ok_or_else(|| "revision order is not topological".to_owned())
            })
            .collect::<Result<_>>()?;
        let parent_unknown: Vec<_> = parents
            .iter()
            .map(|parent| {
                unknown_states
                    .get(*parent)
                    .cloned()
                    .ok_or_else(|| "unknown title history is not topological".to_owned())
            })
            .collect::<Result<_>>()?;
        if !changed.contains(commit) && parent_states.len() == 1 {
            states.insert(commit.to_owned(), Rc::clone(&parent_states[0]));
            unknown_states.insert(commit.to_owned(), Rc::clone(&parent_unknown[0]));
            continue;
        }
        let (snapshot, unknown_titles) = historical_items(&historical_blob(repo, commit)?)
            .map_err(|error| format!("{commit}: {error}"))?;
        let mut counts = HashMap::new();
        for item in &snapshot {
            *counts.entry(item.key.clone()).or_insert(0usize) += 1;
        }
        let replacements = replacement_keys(repo, commit, &parents)?;
        let renamed = changelog_renamed(repo, commit, &parents)?;
        let removed_unparsed = parent_unknown
            .iter()
            .any(|titles| titles.iter().any(|title| !unknown_titles.contains(title)));
        let mut state = BTreeMap::new();
        for item in snapshot {
            let candidates: Vec<&Origin> = parent_states
                .iter()
                .filter_map(|parent| parent.get(&item.key))
                .collect();
            let previously_unparsed = item.key.split_once('\u{1f}').is_some_and(|(title, _)| {
                parent_unknown
                    .iter()
                    .any(|titles| titles.contains(title) || titles.contains(title.trim()))
            });
            let origin = if candidates.is_empty() {
                Origin {
                    commit: commit.to_owned(),
                    ambiguous: renamed
                        || previously_unparsed
                        || removed_unparsed
                        || replacements.contains(&item.key)
                        || counts[&item.key] > 1,
                    reason: if previously_unparsed {
                        Some("item may descend from an unparsed historical entry".to_owned())
                    } else if removed_unparsed {
                        Some(
                            "an unparsed historical entry disappeared in the same commit"
                                .to_owned(),
                        )
                    } else if renamed {
                        Some("CHANGELOG rename or copy makes introduction ambiguous".to_owned())
                    } else {
                        replacements.contains(&item.key).then(|| {
                            "new key appears with a removed key in the same diff hunk".to_owned()
                        })
                    },
                }
            } else {
                let mut inherited = (*candidates[0]).clone();
                if previously_unparsed {
                    inherited.ambiguous = true;
                    inherited.reason =
                        Some("item may descend from an unparsed historical entry".into());
                } else if candidates
                    .iter()
                    .any(|candidate| candidate.commit != inherited.commit)
                    || counts[&item.key] > 1
                {
                    inherited.ambiguous = true;
                    inherited.reason = Some("multiple introductions or duplicate keys".into());
                }
                inherited
            };
            state.insert(item.key, origin);
        }
        states.insert(commit.to_owned(), Rc::new(state));
        unknown_states.insert(commit.to_owned(), Rc::new(unknown_titles));
    }
    let final_state = states.get(head).ok_or("HEAD not in history")?;
    let mut result = HashMap::new();
    for item in current {
        result.insert(
            item.key.clone(),
            final_state.get(&item.key).cloned().unwrap_or(Origin {
                ambiguous: true,
                reason: Some("item absent from HEAD history".into()),
                ..Origin::default()
            }),
        );
    }
    Ok(result)
}

fn tags(repo: &Path, head: &str) -> Result<Vec<(String, String)>> {
    let lines = git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname:lstrip=2) %(objectname)",
            "refs/tags",
        ],
    )?;
    let mut tags = Vec::new();
    for line in lines.lines() {
        let Some((tag, _)) = line.split_once(' ') else {
            continue;
        };
        if !version_tag(tag) {
            continue;
        }
        let peeled = git(repo, &["rev-parse", &format!("refs/tags/{tag}^{{commit}}")])?;
        if !is_ancestor(repo, &peeled, head)? {
            continue;
        }
        tags.push((tag.to_owned(), peeled));
    }
    Ok(tags)
}

fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let output = develop_tools_core::git(repo)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .output()
        .map_err(|error| format!("git merge-base: {error}"))?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!(
            "git merge-base failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )),
    }
}

fn release_tags(identity: &str) -> Result<HashSet<String>> {
    let output = Command::new("gh")
        .args([
            "release",
            "list",
            "--repo",
            identity,
            "--json",
            "tagName,isDraft",
            "--limit",
            "1000",
        ])
        .output()
        .map_err(|error| format!("GitHub Release lookup failed: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "GitHub Release lookup failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let values: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("GitHub Release list invalid: {error}"))?;
    if values.len() == 1000 {
        return Err("GitHub Release list reached its limit; completeness is unknown".into());
    }
    values
        .iter()
        .filter_map(|value| {
            let draft = value["isDraft"].as_bool();
            if draft == Some(true) {
                None
            } else {
                Some(
                    draft
                        .ok_or("GitHub Release has no isDraft".to_owned())
                        .and_then(|_| {
                            value["tagName"]
                                .as_str()
                                .map(str::to_owned)
                                .ok_or("GitHub Release has no tagName".to_owned())
                        }),
                )
            }
        })
        .collect()
}

fn ancestors(repo: &Path, oid: &str) -> Result<HashSet<String>> {
    Ok(git(repo, &["rev-list", oid])?
        .lines()
        .map(str::to_owned)
        .collect())
}

fn first_tag(
    intro: &str,
    tags: &[(String, String)],
    tag_ancestors: &HashMap<String, HashSet<String>>,
) -> std::result::Result<Option<String>, String> {
    let containing: Vec<_> = tags
        .iter()
        .filter(|(_, oid)| tag_ancestors[oid].contains(intro))
        .collect();
    if containing.is_empty() {
        return Ok(None);
    }
    let minimal: Vec<_> = containing
        .iter()
        .filter(|(_, oid)| {
            !containing
                .iter()
                .any(|(_, other)| other != oid && tag_ancestors[oid].contains(other))
        })
        .collect();
    if minimal.len() != 1 {
        return Err("incomparable earliest tags".into());
    }
    Ok(Some(minimal[0].0.clone()))
}

fn plan(options: &Options) -> Result<(Plan, Vec<Item>, Vec<u8>)> {
    let repo = &options.repo;
    validate_repo(repo)?;
    let head = git(repo, &["rev-parse", "HEAD"])?;
    let source_blob = git(repo, &["rev-parse", "HEAD:CHANGELOG.md"])?;
    let source = fs::read(repo.join("CHANGELOG.md")).map_err(|error| error.to_string())?;
    if git(repo, &["hash-object", "CHANGELOG.md"])? != source_blob {
        return Err("CHANGELOG source blob differs from HEAD".into());
    }
    let (current, invalid_lines) = scan_items(&source)?;
    let identity = origin(repo)?;
    validate_stamped(&current, &identity)?;
    let origins = history(repo, &head, &current)?;
    let tags = tags(repo, &head)?;
    let mut tag_ancestors = HashMap::new();
    for (_, oid) in &tags {
        tag_ancestors
            .entry(oid.clone())
            .or_insert(ancestors(repo, oid)?);
    }
    let releases = release_tags(&identity)?;
    let mut planned = Vec::new();
    let mut counts = HashMap::new();
    for item in &current {
        *counts.entry(&item.key).or_insert(0usize) += 1;
    }
    for item in &current {
        let origin = &origins[&item.key];
        let mut reason = origin.reason.clone();
        if origin.ambiguous || counts[&item.key] > 1 {
            reason.get_or_insert_with(|| "ambiguous introduction".into());
        }
        let selected = if reason.is_none() {
            match first_tag(&origin.commit, &tags, &tag_ancestors) {
                Ok(tag) => tag,
                Err(error) => {
                    reason = Some(error);
                    None
                }
            }
        } else {
            None
        };
        let stamp = selected.unwrap_or_else(|| "⏳".to_owned());
        if let Some(legacy) = &item.legacy {
            if legacy == "pending" && stamp != "⏳" {
                reason = Some(format!(
                    "legacy pending stamp conflicts with containing tag {stamp}"
                ));
            } else if legacy != "pending" && legacy != &stamp {
                reason = Some(format!("legacy prod stamp {legacy} conflicts with {stamp}"));
            }
        }
        if let Some(supplement) = item
            .supplement_versions
            .iter()
            .find(|version| *version != &stamp)
        {
            reason = Some(format!(
                "supplement version {supplement} conflicts with {stamp}"
            ));
        }
        if item.stamped {
            reason = None;
        }
        let release_url = (stamp != "⏳" && releases.contains(&stamp))
            .then(|| format!("https://github.com/{identity}/releases/tag/{stamp}"));
        planned.push(PlanItem {
            line: item.line,
            key: item.key.clone(),
            introduction: (!origin.commit.is_empty()).then(|| origin.commit.clone()),
            stamp: (reason.is_none() && !item.stamped).then_some(stamp.clone()),
            proposed_stamp: stamp,
            release_url: (reason.is_none() && !item.stamped)
                .then_some(release_url)
                .flatten(),
            reason,
            resolution_reason: None,
        });
    }
    let manifest =
        fs::read(repo.join(".config/develop-tools.json")).map_err(|error| error.to_string())?;
    let mut releases_sorted: Vec<_> = releases.iter().cloned().collect();
    releases_sorted.sort();
    let hash_input = json!({
        "head": head,
        "sourceBlob": source_blob,
        "origin": identity,
        "manifest": develop_tools_core::managed_block::sha256_hex(&manifest),
        "tags": tags,
        "releases": releases_sorted,
        "items": planned.iter().map(|item| json!([item.line, item.key, item.introduction, item.proposed_stamp, item.stamp, item.reason])).collect::<Vec<_>>()
    });
    let hash_bytes = serde_json::to_vec(&hash_input).map_err(|error| error.to_string())?;
    let plan_hash = develop_tools_core::managed_block::sha256_hex(&hash_bytes);
    let holds = planned.iter().filter(|item| item.reason.is_some()).count();
    let target = rewrite(&source, &current, &planned)?;
    if holds == 0 && invalid_lines.is_empty() {
        validate_rewrite(&source, &target, &current, &identity)?;
    }
    let result = Plan {
        head,
        source_blob,
        plan_hash,
        origin: identity,
        items: planned,
        holds,
        invalid: invalid_lines.len(),
        invalid_lines,
        source_bytes: source.len(),
        target_bytes: target.len(),
        release_fetched_at: format!("{:?}", std::time::SystemTime::now()),
        available_tags: tags.into_iter().map(|(tag, _)| tag).collect(),
        releases,
    };
    Ok((result, current, source))
}

fn validate_rewrite(source: &[u8], target: &[u8], current: &[Item], identity: &str) -> Result<()> {
    let parsed = items(target)?;
    if source.iter().filter(|byte| **byte == b'\n').count()
        != target.iter().filter(|byte| **byte == b'\n').count()
        || parsed.len() != current.len()
    {
        return Err("rewrite changed line or item count".into());
    }
    if parsed.iter().any(|item| !item.stamped) {
        return Err("rewrite left an old item form".into());
    }
    validate_stamped(&parsed, identity)
}

fn rewrite(source: &[u8], items: &[Item], planned: &[PlanItem]) -> Result<Vec<u8>> {
    let mut result = Vec::with_capacity(source.len());
    let mut offset = 0;
    for (index, line) in source.split_inclusive(|byte| *byte == b'\n').enumerate() {
        let number = index + 1;
        if let Some((item, plan)) = items
            .iter()
            .zip(planned)
            .find(|(item, _)| item.line == number)
        {
            if let Some(stamp) = &plan.stamp {
                let line_text = std::str::from_utf8(line).map_err(|error| error.to_string())?;
                if line_text.get(..item.date_end).is_none()
                    || line_text.get(item.suffix_end..).is_none()
                {
                    return Err("invalid item byte offsets".into());
                }
                result.extend_from_slice(&line[..item.date_end]);
                result.extend_from_slice(b"/");
                if let Some(url) = &plan.release_url {
                    result.extend_from_slice(format!("[{stamp}]({url})").as_bytes());
                } else {
                    result.extend_from_slice(stamp.as_bytes());
                }
                let remainder = &line_text[item.date_end..item.suffix_end];
                let preserved = item.legacy_segment.as_ref().map_or_else(
                    || remainder.to_owned(),
                    |segment| remainder.replacen(segment, "", 1),
                );
                result.extend_from_slice(preserved.as_bytes());
                result.extend_from_slice(&line[item.suffix_end..]);
            } else {
                result.extend_from_slice(line);
            }
        } else {
            result.extend_from_slice(line);
        }
        offset += line.len();
    }
    if offset != source.len() {
        return Err("source line traversal mismatch".into());
    }
    Ok(result)
}

fn resolve(options: &Options, plan: &mut Plan) -> Result<()> {
    let Some(path) = &options.resolutions else {
        return Ok(());
    };
    let file: ResolutionFile =
        serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
            .map_err(|error| format!("resolution file invalid: {error}"))?;
    if file.source_blob != plan.source_blob || file.plan_hash != plan.plan_hash {
        return Err("resolution sourceBlob or planHash mismatch".into());
    }
    let mut seen = BTreeSet::new();
    for resolution in file.items {
        if !seen.insert((resolution.line, resolution.key.clone())) {
            return Err("duplicate resolution".into());
        }
        let item = plan
            .items
            .iter_mut()
            .find(|item| item.line == resolution.line && item.key == resolution.key)
            .ok_or("resolution item key or line mismatch")?;
        if item.reason.is_none() || resolution.reason.trim().is_empty() {
            return Err("resolution applies only to a hold and needs a reason".into());
        }
        if resolution.stamp != "⏳" && !plan.available_tags.contains(&resolution.stamp) {
            return Err("resolution stamp is not a reachable version tag".into());
        }
        if item.proposed_stamp != resolution.stamp {
            eprintln!(
                "resolution overrides history stamp {} with {}",
                item.proposed_stamp, resolution.stamp
            );
        }
        item.release_url = (resolution.stamp != "⏳" && plan.releases.contains(&resolution.stamp))
            .then(|| {
                format!(
                    "https://github.com/{}/releases/tag/{}",
                    plan.origin, resolution.stamp
                )
            });
        item.stamp = Some(resolution.stamp);
        item.reason = None;
        item.resolution_reason = Some(resolution.reason);
    }
    plan.holds = plan
        .items
        .iter()
        .filter(|item| item.reason.is_some())
        .count();
    Ok(())
}

fn execute(options: Options) -> Result<Plan> {
    let (mut plan, current, source) = plan(&options)?;
    resolve(&options, &mut plan)?;
    if options.write {
        if options.plan_hash.as_deref() != Some(plan.plan_hash.as_str()) {
            return Err("plan hash mismatch; re-run --check".into());
        }
        if plan.holds != 0 || plan.invalid != 0 {
            return Err(format!(
                "cannot write with {} holds and {} invalid items",
                plan.holds, plan.invalid
            ));
        }
        let target = rewrite(&source, &current, &plan.items)?;
        validate_rewrite(&source, &target, &current, &plan.origin)?;
        if target != source {
            let path = options.repo.join("CHANGELOG.md");
            let latest = fs::read(&path).map_err(|error| error.to_string())?;
            if latest != source {
                return Err("CHANGELOG changed before write".into());
            }
            let mut temp = tempfile::NamedTempFile::new_in(&options.repo)
                .map_err(|error| error.to_string())?;
            let permissions = fs::metadata(&path)
                .map_err(|error| error.to_string())?
                .permissions();
            temp.as_file_mut()
                .set_permissions(permissions)
                .map_err(|error| error.to_string())?;
            temp.write_all(&target).map_err(|error| error.to_string())?;
            temp.as_file_mut()
                .sync_all()
                .map_err(|error| error.to_string())?;
            temp.persist(&path).map_err(|error| error.to_string())?;
        }
    }
    let target = rewrite(&source, &current, &plan.items)?;
    plan.target_bytes = target.len();
    Ok(plan)
}

pub fn run(args: Vec<String>) -> i32 {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "Usage: changelog-migrate --repo=<absolute-path> [--check] [--json]\n       changelog-migrate --repo=<absolute-path> --write --plan-hash=<hash> [--resolutions=<json>] [--json]\n\nDefault: dry-run; prints a migration plan without writing.\nWrites: only the target repo's CHANGELOG.md, when the fixed plan matches and no holds remain."
        );
        return 0;
    }
    let options = match parse_options(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    let json_output = options.json;
    let check = options.check;
    match execute(options) {
        Ok(plan) => {
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&plan).expect("plan is serializable")
                );
            } else {
                println!(
                    "{} items, {} holds, {} invalid, plan {}",
                    plan.items.len(),
                    plan.holds,
                    plan.invalid,
                    plan.plan_hash
                );
            }
            if check && plan.invalid != 0 { 1 } else { 0 }
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}
