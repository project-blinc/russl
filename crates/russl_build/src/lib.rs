//! Compile russl shader files from a build script.
//!
//! A file and the files its `use` items reach compile together, as one program. The directory
//! holding the root file is the module tree's root unless [`compile_in`] names another:
//! `shaders/sdf_core.russl` imports `shaders/common.russl` with `use super::common::*;` or
//! `use crate::common::*;`. Warnings go to Cargo as `cargo:warning`; errors come back rendered
//! against the file they are in.

use std::path::Path;

pub use russl_compiler::{
    Artifacts, ConstValue, Constant, Diagnostic, Extension, NoExtension, Program, Severity,
};

/// Compile `root` and what it imports, with the module tree rooted at `root`'s directory.
/// Tells Cargo to rerun the build script when any of the files changes.
pub fn compile(root: impl AsRef<Path>) -> Result<Artifacts, String> {
    let root = root.as_ref();
    let dir = root.parent().unwrap_or(Path::new("."));
    compile_in(dir, root)
}

/// Compile `root` and what it imports, with the module tree rooted at `dir`.
pub fn compile_in(dir: impl AsRef<Path>, root: impl AsRef<Path>) -> Result<Artifacts, String> {
    compile_with(dir, root, &[])
}

/// As [`compile_in`], with values for some of the root's pipeline constants.
pub fn compile_with(
    dir: impl AsRef<Path>,
    root: impl AsRef<Path>,
    constants: &[Constant],
) -> Result<Artifacts, String> {
    compile_ext(dir, root, constants, &russl_compiler::NoExtension)
}

/// As [`compile_with`], with a framework's [`Extension`]: its prelude, externs, builtins and
/// rules.
pub fn compile_ext(
    dir: impl AsRef<Path>,
    root: impl AsRef<Path>,
    constants: &[Constant],
    ext: &dyn Extension,
) -> Result<Artifacts, String> {
    let (dir, root) = (dir.as_ref(), root.as_ref());
    // The tree too, so a file an import is waiting for reruns the build when it appears.
    println!("cargo:rerun-if-changed={}", dir.display());
    println!("cargo:rerun-if-changed={}", root.display());
    let (program, diagnostics) = Program::load(dir, root)?;
    for f in &program.files[1..] {
        println!("cargo:rerun-if-changed={}", f.path.display());
    }
    if !diagnostics.is_empty() {
        return Err(russl_compiler::render_program(
            &diagnostics,
            &program,
            false,
        ));
    }
    match program.compile_ext(constants, ext) {
        Ok(artifacts) => {
            for w in &artifacts.warnings {
                let at = w.span.start();
                println!(
                    "cargo:warning={}:{}:{}: {}",
                    program.file_of(w.span).path.display(),
                    at.line,
                    at.column + 1,
                    w.message
                );
            }
            Ok(artifacts)
        }
        Err(d) => Err(russl_compiler::render_program(&d, &program, false)),
    }
}
