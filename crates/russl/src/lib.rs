//! russl (pronounced Russel), the Rust Shader Language.
//!
//! Shaders written in Rust syntax, compiled at build time to WGSL, with the native shader
//! languages to follow, so no shader compiler runs on the user's machine.

pub use russl_macros::russl;

#[doc(hidden)]
pub use russl_macros::__russl_continue;
