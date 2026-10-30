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
    ("src/preflight.rs", include_str!("preflight.rs")),
    ("src/report.rs", include_str!("report.rs")),
    ("src/app.rs", include_str!("app.rs")),
    ("src/theme.rs", include_str!("theme.rs")),
    ("src/headless.rs", include_str!("headless.rs")),
    ("src/config.rs", include_str!("config.rs")),
    ("src/mock_cpe.rs", include_str!("mock_cpe.rs")),
    ("src/scrub.rs", include_str!("scrub.rs")),
    ("src/share.rs", include_str!("share.rs")),
    ("src/ui/mod.rs", include_str!("ui/mod.rs")),
    ("src/ui/tiles.rs", include_str!("ui/tiles.rs")),
];

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
        assert!(
            source.contains(&format!("fn {name}(")),
            "{name} does not exist in {file}: the contract cites a guard that is gone"
        );
    }
    assert!(rows >= 10, "the contracts table lost its rows ({rows})");
}
