//! The same shader file under two Rust modules compiles against each module's own imports.

mod bright;
mod dim;

#[test]
fn a_file_included_twice_sees_the_imports_beside_it() {
    assert!(
        bright::shader::WGSL.contains("0.75"),
        "{}",
        bright::shader::WGSL
    );
    assert!(dim::shader::WGSL.contains("0.25"), "{}", dim::shader::WGSL);
    assert!(!dim::shader::WGSL.contains("0.75"));
}
