//! model 名は用途の表（`src/purpose.rs`）にだけ書く。呼び出し側も model 名で分岐しない。

use std::path::Path;

const TABLE: &str = "src/purpose.rs";

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn model_names_appear_only_in_the_purpose_table() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    assert!(files.iter().any(|path| path.ends_with(TABLE)));

    for path in files {
        let relative = path.strip_prefix(root).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let mentions = text.matches("\"claude-").count();
        if relative == Path::new(TABLE) {
            assert_eq!(
                mentions,
                island_llm::purpose::PURPOSES.len(),
                "one model literal per table row"
            );
        } else {
            assert_eq!(mentions, 0, "{} names a model", relative.display());
        }
    }
}

#[test]
fn every_table_row_names_a_model() {
    for purpose in island_llm::purpose::PURPOSES {
        assert!(purpose.model.starts_with("claude-"), "{}", purpose.name);
    }
    let names: Vec<_> = island_llm::purpose::PURPOSES
        .iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(names, ["digest", "release-notes"]);
}
