use develop_tools_core::cli_help::Help;

#[test]
fn required_sections_and_optional_sections_have_a_stable_order() {
    let help = Help::new(
        "sample [--check]",
        "Sample operation.",
        "applies changes",
        "--check: none; otherwise: target files",
    )
    .exit("0 success; 1 failure")
    .examples(&["sample --check"])
    .render();

    assert_eq!(
        help,
        "Usage: sample [--check]\n\nSample operation.\n\nDefault: applies changes\nWrites: --check: none; otherwise: target files\nExit: 0 success; 1 failure\n\nExamples:\n  sample --check\n"
    );
}

#[test]
fn optional_sections_can_be_omitted() {
    assert_eq!(
        Help::new("sample", "Sample.", "read-only", "none").render(),
        "Usage: sample\n\nSample.\n\nDefault: read-only\nWrites: none\n"
    );
}
