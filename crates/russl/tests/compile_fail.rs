//! What rustc prints for a shader error: at the offending token, in the author's words. The
//! expected output is stable rustc's; nightly joins spans across tokens and underlines more.

#[test]
fn errors_land_on_the_offending_token() {
    let version = std::process::Command::new(std::env::var("RUSTC").unwrap_or("rustc".into()))
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if version.contains("nightly") {
        eprintln!("{version}: the expected output is stable's; skipped");
        return;
    }
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}
