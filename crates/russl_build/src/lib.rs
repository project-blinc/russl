//! Compile russl shader files from a build script.
//!
//! A proc macro sees only its own tokens, so shaders that span files or crates are compiled here,
//! where the driver can read every file it needs. Warnings go to Cargo as `cargo:warning`;
//! errors come back rendered against the source.

use std::path::Path;

pub use russl_compiler::{Artifacts, Diagnostic, Severity};

/// Compile one russl source file. Tells Cargo to rerun the build script when it changes.
pub fn compile_file(path: impl AsRef<Path>) -> Result<Artifacts, String> {
    let path = path.as_ref();
    println!("cargo:rerun-if-changed={}", path.display());
    let name = path.display().to_string();
    let source = std::fs::read_to_string(path).map_err(|e| format!("{name}: {e}"))?;
    let tokens: proc_macro2::TokenStream = source.parse().map_err(|e| format!("{name}: {e}"))?;
    match russl_compiler::compile(tokens) {
        Ok(artifacts) => {
            for w in &artifacts.warnings {
                let at = w.span.start();
                println!(
                    "cargo:warning={name}:{}:{}: {}",
                    at.line,
                    at.column + 1,
                    w.message
                );
            }
            Ok(artifacts)
        }
        Err(diagnostics) => Err(russl_compiler::render(&diagnostics, &name, &source, false)),
    }
}
