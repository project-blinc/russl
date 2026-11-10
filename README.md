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

A `#[pipeline]` constant is specialised at build time, never at run time: a module holding
`#![specialize(path::to::shader)]` and `const NAME: T = value;` items expands to that shader's
artifacts with those values.

## Usage

```toml
[dependencies]
russl = "0.3"
```

A `russl!` block is one shader module. One with entry points expands to `WGSL`, `MSL`, `HLSL`,
`SPIRV` and `GLSL` consts; one without is a library that others `use`.

```rust
mod sdf {
    russl::russl! {
        /// The distance from `p` to a box of half-size `b` with corner radius `r`.
        pub fn rounded_box(p: Vec2, b: Vec2, r: f32) -> f32 {
            let q = p.abs() - b + r;
            q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.0) - r
        }
    }
}

mod card {
    russl::russl! {
        use super::sdf::rounded_box;

        struct Card { size: Vec2, radius: f32, color: Vec4 }

        struct VsOut {
            #[builtin(position)] pos: Vec4,
            #[location(0)] local: Vec2,
        }

        #[group(0)] #[binding(0)] static CARD: Uniform<Card>;
        #[pipeline] const SOFT_EDGE: bool = true;

        const CORNERS: [Vec2; 6] = [
            Vec2::new(-1.0, -1.0), Vec2::new(1.0, -1.0), Vec2::new(1.0, 1.0),
            Vec2::new(-1.0, -1.0), Vec2::new(1.0, 1.0), Vec2::new(-1.0, 1.0),
        ];

        #[vertex]
        fn vs(#[builtin(vertex_index)] vi: u32) -> VsOut {
            let corner = CORNERS[vi];
            VsOut { pos: Vec4::new(corner, 0.0, 1.0), local: corner * CARD.size * 0.5 }
        }

        #[fragment]
        #[location(0)]
        fn fs(input: VsOut) -> Vec4 {
            let d = rounded_box(input.local, CARD.size * 0.5, CARD.radius);
            if d > 1.0 {
                discard!();
            }
            let aa = if SOFT_EDGE { d.fwidth() } else { 0.0001 };
            let alpha = 1.0 - d.smoothstep(-aa, aa);
            CARD.color * alpha
        }
    }
}

mod card_hard {
    russl::russl! {
        #![specialize(super::card)]
        const SOFT_EDGE: bool = false;
    }
}
```

`card::WGSL` goes to wgpu or WebGPU as it is, `card::MSL` to Metal, `card::SPIRV` to Vulkan, and
so on; `card_hard` is the same shader with `SOFT_EDGE` fixed to `false`. A mistake is reported
by rustc at the token that caused it. Building a `Vec4` from a `Vec2` and one float:

```text
error: `Vec4::new` needs 4 components, found 3
 --> src/card.rs:7:13
  |
7 |             Vec4::new(local, d)
  |             ^^^^
```

### The host side

Each module also expands to what the host needs to feed it. A struct gets a Rust twin holding
the bytes a buffer holds, with WGSL's offsets and padding: `Scene::new(..)`, `Scene::zeroed()`,
`scene.as_bytes()`, `Scene::SIZE` and `Scene::TINT_OFFSET`, with the layout asserted when the
crate builds. Vectors are arrays (`Vec3` is `[f32; 3]`, but `[f32; 4]` inside an array, as its
stride is), and a struct shared by several shaders lives in one library module whose twin they
all use. `#![bytemuck]` adds `bytemuck::Pod`. Bindings become `SCENE_GROUP`, `SCENE_BINDING` and
`SCENE_SIZE`, the pipeline constants as built `CONSTANTS` (wgpu's `(name, f64)` list), and a
vertex entry point's inputs `VS_INPUTS` as (name, location, `wgpu::VertexFormat` variant).

### Shader files

Shaders can also live in `.russl` files, compiled from `build.rs` by `russl_build` or checked by
the `russl` command. A directory is a module tree, laid out as Rust lays out modules:
`shaders/shapes/ring.russl` (or `shaders/shapes/ring/mod.russl`) is `crate::shapes::ring`, and a
file imports others with `use crate::...`, `use super::...` or `use self::...`.

```rust
// build.rs
fn main() {
    let card = russl_build::compile("shaders/card.russl").unwrap_or_else(|e| panic!("{e}"));
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::write(out.join("card.wgsl"), &card.wgsl).unwrap();
}
```

```sh
russl check shaders/card.russl --deny-warnings   # what CI runs
russl emit shaders/card.russl msl                # or wgsl, hlsl, glsl, spirv
```

### Extensions

A framework adds to russl without changing it, through `russl_compiler::Extension`:

- a **prelude**, a library module whose public items every module of its shaders sees;
- **extern functions**: a shader declares `extern { fn aux(i: u32) -> Vec4; }` and the
  extension defines it, so one shader links against a per-backend or per-tier implementation;
- **builtins** it lowers to naga itself, for what russl source cannot say;
- **rules** over the checked program: its bindings, entry points and pipeline constants.

Its shader macro is `russl_expand` with that extension, two functions long:

```rust
#[proc_macro]
pub fn my_shader(input: TokenStream) -> TokenStream {
    expander().expand(input.into()).into()
}

#[doc(hidden)]
#[proc_macro]
pub fn __my_continue(input: TokenStream) -> TokenStream {
    expander().resume(input.into()).into()
}

fn expander() -> russl_expand::Expander<'static> {
    russl_expand::Expander {
        continuation: quote::quote! { ::my_macros::__my_continue },
        extension: &MyExtension,
    }
}
```

Modules written with `russl!` and with a framework's macro import each other; a program
compiles with its root's extension. `russl_build::compile_ext` takes one too.

## Design

- **Rust syntax**: `fn`, `struct`, `const`, `let`, `if`/`else`, `match`, `for` over ranges,
  method-call builtins (`v.dot(w)`, `x.clamp(a, b)`, `x.fwidth()`), texture methods
  (`atlas.sample_level(sampler, uv, 0.0)`, `t.load(coord, 0)`, `t.dimensions()`), `discard!()`,
  swizzles on both sides of `=` (`c.xz = v`), arrays sized by constants (`[f32; TAPS * 2]`) and
  repeated (`[0.0; N]`), `f16` and `HVec*`, Rust operators and casts.
- **Generics**: `fn lerp<T: Float>(..)`, `fn blur<const N: u32>(w: [f32; N])`, `struct Pair<T>`
  and `struct Ring<const N: u32>`, built once per distinct set of arguments. A call infers them
  (through `Pair<T>` too), or names them: `zeros::<4>()`, `Pair::<Vec2> { .. }`. Bounds are
  `Float`, `Int`, `Numeric`, `Scalar` and `Vector`.
- **WGSL's rules, in Rust's words**: entry-point IO, bindings, and the uniform buffer layout
  (16-byte array strides and struct alignment) are checked before naga sees the module, with
  the error at the field.
- **Every target, every compile**: WGSL for WebGPU, MSL for Metal, HLSL for Direct3D 12, SPIR-V
  for Vulkan and GLSL ES for WebGL2, all at build time through naga's backends. A platform takes the
  artifact it needs and never compiles a shader at run time. MSL targets version 2.1; a shader
  that needs what WebGL2 lacks, such as storage buffers, has no GLSL.
- **Native binding models**: MSL binds resources as wgpu's Metal backend does, so wgpu's
  passthrough shader modules take it as it is; HLSL uses register = binding and space = group;
  SPIR-V uses set = group. `Artifacts::reflection.native` lists each target's slots, and CI
  checks every target with its own compiler (spirv-val, DXC, the Metal compiler, glslang).
- **Modules and reuse**: a module without entry points is a library; `use` imports its `pub`
  functions, types and bindings by path, across files and crates, and only what an entry point
  reaches is emitted. A module that other crates import, and every module it imports, starts
  with `#![export]`; any other stays inside its Rust module, so one file can be included under
  several Rust modules (`#[path]`), each compiling against the imports beside it. Inspired by the
  export, import and linking of Heaps' HXSL.
- **Errors at your token**: never a line number in generated WGSL.
- **Extensions**: a framework adds its own prelude, externs, builtins and rules without changing
  the core.

Not goals: compiling arbitrary Rust (that is [rust-gpu](https://github.com/Rust-GPU/rust-gpu)),
runtime compilation, or features WGSL cannot express.

`fixtures/blinc_sdf` is Blinc's SDF core shader written in russl, with its simple pipeline as a
specialisation. Blinc checks that pipeline against its own, pixel for pixel.

## Crates

| crate | role |
|---|---|
| `russl` | the facade: re-exports the macro and the small runtime types |
| `russl_compiler` | the compiler, shared by everything else; no GPU dependency |
| `russl_macros` | the `russl!` proc macro |
| `russl_expand` | the expansion behind `russl!`, for a framework's own macro with its extension |
| `russl_build` | compiles shader files and what they import from `build.rs` |
| `russl_cli` | `russl check` and `russl emit` over shader files, for CI and for reading the output |

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
