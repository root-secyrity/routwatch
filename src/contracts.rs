//! Keeps `docs/contracts.md` honest.
//!
//! The document says "this rule is guarded by this test". A document that can
//! drift is worse than no document, so this test reads the table back and
//! requires every row to name a file in this repository and a test function
//! that actually exists in it. Renaming or deleting a guard therefore fails the
//! build instead of silently retiring a promise.

/// Every source file the table is allowed to point at. A new contract that
/// lives in another file has to be added here, which is the point: the table
/// cannot cite a file nobody checked.
const SOURCES: &[(&str, &str)] = &[
    ("src/error.rs", include_str!("error.rs")),
    ("src/preflight.rs", include_str!("preflight.rs")),
    ("src/report.rs", include_str!("report.rs")),
    ("src/app.rs", include_str!("app.rs")),
    ("src/theme.rs", include_str!("theme.rs")),
    ("src/headless.rs", include_str!("headless.rs")),
    ("src/config.rs", include_str!("config.rs")),
    ("src/mock_cpe.rs", include_str!("mock_cpe.rs")),
    ("src/scrub.rs", include_str!("scrub.rs")),
    ("src/share.rs", include_str!("share.rs")),
    ("src/output.rs", include_str!("output.rs")),
    ("src/worker.rs", include_str!("worker.rs")),
    ("src/ui/mod.rs", include_str!("ui/mod.rs")),
    ("src/ui/tiles.rs", include_str!("ui/tiles.rs")),
];

/// The 1-based line that starts `fn name(`, or None when the file has no such
/// function.
fn fn_line(text: &str, name: &str) -> Option<usize> {
    let needle = format!("fn {name}(");
    text.lines()
        .position(|l| l.trim_start().starts_with(&needle))
        .map(|i| i + 1)
}

/// True when the function at `line` (1-based) is a test: the `#[test]`
/// attribute sits a few lines above it, past any doc comment or a second
/// attribute such as `#[should_panic]`.
fn carries_test_attr(text: &str, line: usize) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    let end = line.saturating_sub(1).min(lines.len());
    let start = end.saturating_sub(4);
    lines[start..end].iter().any(|l| l.trim() == "#[test]")
}

#[test]
fn every_contract_names_a_test_that_exists() {
    let doc = include_str!("../docs/contracts.md");
    let mut rows = 0;
    for line in doc.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        // A contract row is `| contract | `test` | `file` |` - the header, the
        // `| --- |` separator and the prose all lack backticks in cells 2 and 3.
        if cells.len() < 5 || !cells[2].starts_with('`') || !cells[3].starts_with('`') {
            continue;
        }
        rows += 1;
        let name = cells[2].trim_matches('`');
        let file = cells[3].trim_matches('`');
        let source = SOURCES
            .iter()
            .find(|(path, _)| *path == file)
            .map(|(_, text)| *text)
            .unwrap_or_else(|| panic!("{name}: {file} is not a checked source file"));
        let line = fn_line(source, name).unwrap_or_else(|| {
            panic!("{name} does not exist in {file}: the contract cites a guard that is gone")
        });
        // A row promises a guard, so the cited function must be a test: a
        // helper or a call site with the right name guards nothing.
        assert!(
            carries_test_attr(source, line),
            "{name} in {file} carries no #[test]: the contract cites a function, not a guard"
        );
    }
    assert!(rows >= 10, "the contracts table lost its rows ({rows})");
}

#[test]
fn a_cited_guard_must_carry_the_test_attribute() {
    let guarded = "/// What it proves.\n#[test]\nfn a_guard() {\n    assert!(true);\n}\n";
    let helper = "fn a_guard() -> bool {\n    true\n}\n";
    let second_attr = "#[test]\n#[should_panic]\nfn a_guard() {\n    panic!()\n}\n";

    let line = fn_line(guarded, "a_guard").expect("the guarded fixture names the function");
    assert!(carries_test_attr(guarded, line));
    let line = fn_line(helper, "a_guard").expect("the helper fixture names the function");
    assert!(!carries_test_attr(helper, line), "a helper is not a guard");
    let line = fn_line(second_attr, "a_guard").expect("the attribute fixture names it");
    assert!(carries_test_attr(second_attr, line));
    assert_eq!(fn_line(helper, "another_guard"), None);
}
