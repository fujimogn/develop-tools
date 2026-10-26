//! Read-only inspection of all ADRs in the current working tree.

use develop_adr::{validate_archive, validate_sanitized_archive};
use develop_core::{cli_help::Help, develop_tools_manifest};
use develop_staged::adr_standard::{ArchiveValidators, Options, inspect};
use std::{fs, path::Path};

const BASELINE_PATH: &str = "scripts/develop-tools/adr-template.md";

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        print!("{}", Help::new(
            "inspect-adr [--help]",
            "inspect-adr — inspect all working-tree ADRs.",
            "Run from the repository root. Reads .config/develop-tools.json, docs/adr and scripts/develop-tools/adr-template.md; no Git index is required.",
            "None. Files and the Git index are never modified.",
        ).exit("0 = no findings, 1 = documentation violations, 2 = argument or input-reading failure").render());
        return;
    }
    if !args.is_empty() {
        fatal("expected no arguments (or --help)");
    }
    let root = std::env::current_dir().unwrap_or_else(|error| fatal(&format!("cwd: {error}")));
    let path = develop_tools_manifest::locate(&root)
        .unwrap_or_else(|error| fatal(&error))
        .unwrap_or_else(|| fatal("missing .config/develop-tools.json ADR numbering contract"));
    let text = read(&root.join(path));
    let value =
        serde_json::from_str(&text).unwrap_or_else(|error| fatal(&format!("{path}: {error}")));
    let contract = develop_tools_manifest::adr_contract(&value)
        .unwrap_or_else(|error| fatal(&error))
        .unwrap_or_else(|| fatal("missing ADR numbering contract fields"));
    let baseline = read(&root.join(BASELINE_PATH));

    // The shared inspector reports documentation defects. Check I/O first so
    // inaccessible or invalid UTF-8 inputs cannot become a successful audit.
    preflight(&root).unwrap_or_else(|error| fatal(&error));
    let issues = inspect(
        &Options {
            project_root: &root,
            canonical_template: &baseline,
            migrated_through: contract.migrated_through,
            reserved_numbers: &contract.reserved_numbers,
            sanitized_numbers: &contract.sanitized_numbers,
        },
        &ArchiveValidators {
            archive: &validate_archive,
            sanitized: &validate_sanitized_archive,
        },
    );
    for issue in &issues {
        eprintln!("{}: {}", issue.code, issue.message);
    }
    if !issues.is_empty() {
        std::process::exit(1);
    }
}

fn preflight(root: &Path) -> Result<(), String> {
    let dir = root.join("docs/adr");
    let entries = fs::read_dir(&dir).map_err(|error| format!("docs/adr: {error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("docs/adr: {error}"))?;
        if entry.path().extension().is_some_and(|ext| ext == "md") {
            fs::read_to_string(entry.path())
                .map_err(|error| format!("{}: {error}", entry.path().display()))?;
        }
    }
    Ok(())
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| fatal(&format!("{}: {error}", path.display())))
}
fn fatal(message: &str) -> ! {
    eprintln!("error: {message}");
    std::process::exit(2);
}
