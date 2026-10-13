/// Keep the production child allowlist independent of development-tool state.
/// macOS /usr/bin/python3 is an Xcode shim; a fixture may select a real local
/// interpreter without forwarding DEVELOPER_DIR or the parent's PATH to workers.
pub(crate) fn python_fixture(source: &str) -> String {
    let python = std::env::var("WRITER_TEST_PYTHON").unwrap_or_else(|_| "/usr/bin/python3".into());
    assert!(
        std::path::Path::new(&python).is_absolute() && !python.chars().any(char::is_whitespace)
    );
    source.replacen("#!/usr/bin/python3", &format!("#!{python}"), 1)
}
