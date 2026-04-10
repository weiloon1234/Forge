#[test]
fn raw_string_semantic_identifiers_are_rejected() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/raw_string_semantic_ids.rs");
}

#[test]
fn numeric_columns_reject_string_shortcuts() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/numeric_string_shortcuts.rs");
}
