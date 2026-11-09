//! Compile russl shader files from a build script.
//!
//! A proc macro sees only its own tokens, so shaders that span files or crates are compiled here,
//! where the driver can read every file it needs.

use std::path::Path;

pub use russl_compiler::{Artifacts, Error};

/// Compile one russl source file. Tells Cargo to rerun the build script when it changes.
pub fn compile_file(path: impl AsRef<Path>) -> Result<Artifacts, String> {
    let path = path.as_ref();
    println!("cargo:rerun-if-changed={}", path.display());
    let source = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let tokens: proc_macro2::TokenStream = source
        .parse()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    russl_compiler::compile(tokens).map_err(|e| format!("{}: {e}", path.display()))
}
