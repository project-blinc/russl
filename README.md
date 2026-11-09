# russl

**russl** (pronounced *Russel*) is the Rust Shader Language: shaders written in Rust syntax and
compiled at build time, so the GPU code reads like the rest of your crate and no shader compiler
runs on the user's machine.

Rust and WGSL already share most of their syntax. russl closes the gap with a macro and a compiler:
source is parsed with `syn`, type-checked, lowered to a [naga](https://github.com/gfx-rs/wgpu/tree/trunk/naga)
module, validated, and emitted as consts.

russl has its own front end: identifiers are interned, types live in a hash-consed arena, and an
analyzer types every expression (with WGSL's abstract literals), checks places and mutability,
entry-point inputs and outputs and resource bindings, and warns on Rust's naming conventions and
unused bindings. The checked module is lowered straight to naga's IR; errors point at your tokens,
and the CLI renders them with source snippets.

Modules import each other with `use`, within a crate and across crates: a module without entry
points is a library, `pub` items are its interface, and a shader carries only what its entry
points reach.

## Design

- **Rust syntax**: `fn`, `struct`, `const`, `let`, `if`/`else`, `match`, `for` over ranges,
  method-call builtins (`v.dot(w)`, `x.clamp(a, b)`), Rust operators and casts.
- **Every target, every compile**: WGSL for WebGPU, MSL for Metal, HLSL for Direct3D 12, SPIR-V
  for Vulkan and GLSL ES for WebGL2, all at build time through naga's backends. A platform takes the
  artifact it needs and never compiles a shader at run time.
- **Modules and reuse**: a module without entry points is a library; `use` imports its `pub`
  functions, types and bindings by path, across files and crates, and only what an entry point
  reaches is emitted. Inspired by the export, import and linking of Heaps' HXSL.
- **Errors at your token**: never a line number in generated WGSL.
- **Extensions**: a framework adds its own externs, preludes and binding rules without changing
  the core.

Not goals: compiling arbitrary Rust (that is [rust-gpu](https://github.com/Rust-GPU/rust-gpu)),
runtime compilation, or features WGSL cannot express.

## Crates

| crate | role |
|---|---|
| `russl` | the facade: re-exports the macro and the small runtime types |
| `russl_compiler` | the compiler, shared by everything else; no GPU dependency |
| `russl_macros` | the `russl!` proc macro |
| `russl_build` | compiles shader files from `build.rs`, for libraries spanning files and crates |
| `russl_cli` | `russl check` and `russl emit`, for CI and for reading what a macro produced |

naga is pinned to the version [Blinc](https://github.com/project-blinc/Blinc)'s wgpu uses and moves
with it.

## Tracking

Plans, bugs and work in progress live in [git-bug](https://github.com/git-bug/git-bug), in the repository itself (`refs/bugs/*`):

```sh
git-bug pull bugs   # fetch the issues
git-bug bug         # list them
```

## License

Apache-2.0, see [LICENSE](LICENSE).
