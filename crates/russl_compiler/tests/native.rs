//! Every corpus shader's native artifacts through the native compilers: spirv-val for SPIR-V,
//! DXC for HLSL, the Metal compiler for MSL and glslangValidator for GLSL ES. A compiler that is
//! not installed is skipped; `RUSSL_REQUIRE_VALIDATORS` lists those that must be there, as CI
//! sets it (`spirv-val,dxc,glslang` or `metal`).

use russl_compiler::{Artifacts, Stage, compile};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Tool {
    name: &'static str,
    available: bool,
}

fn required() -> Vec<String> {
    std::env::var("RUSSL_REQUIRE_VALIDATORS")
        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default()
}

fn tool(name: &'static str, probe: &mut Command) -> Tool {
    let available = probe.output().is_ok_and(|o| o.status.success());
    if !available && required().iter().any(|r| r == name) {
        panic!("{name} is required (RUSSL_REQUIRE_VALIDATORS) and not available");
    }
    if !available {
        eprintln!("{name} not available: skipped");
    }
    Tool { name, available }
}

fn run(what: &str, cmd: &mut Command) {
    let out = cmd.output().unwrap_or_else(|e| panic!("{what}: {e}"));
    assert!(
        out.status.success(),
        "{what} rejects it:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn corpus() -> Vec<(String, Artifacts)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "russl"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&p).unwrap();
            let a = compile(text.parse().unwrap()).unwrap_or_else(|d| {
                panic!(
                    "{name}: {:?}",
                    d.iter().map(|d| d.to_string()).collect::<Vec<_>>()
                )
            });
            (name, a)
        })
        .collect()
}

fn out_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("russl_native_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn spirv_passes_spirv_val() {
    let t = tool("spirv-val", Command::new("spirv-val").arg("--version"));
    if !t.available {
        return;
    }
    let dir = out_dir();
    for (name, a) in corpus() {
        let path = dir.join(format!("{name}.spv"));
        let bytes: Vec<u8> = a.spirv.iter().flat_map(|w| w.to_le_bytes()).collect();
        std::fs::write(&path, bytes).unwrap();
        run(
            &format!("{} on {name}", t.name),
            Command::new("spirv-val")
                .args(["--target-env", "vulkan1.1"])
                .arg(&path),
        );
    }
}

#[test]
fn hlsl_compiles_with_dxc() {
    let t = tool("dxc", Command::new("dxc").arg("--version"));
    if !t.available {
        return;
    }
    let dir = out_dir();
    for (name, a) in corpus() {
        let path = dir.join(format!("{name}.hlsl"));
        std::fs::write(&path, &a.hlsl).unwrap();
        let half = a.wgsl.contains("enable f16;");
        for (entry, stage) in entry_points(&a) {
            let model = if half { "6_2" } else { "6_0" };
            let profile = match stage {
                Stage::Vertex => format!("vs_{model}"),
                Stage::Fragment => format!("ps_{model}"),
                Stage::Compute => format!("cs_{model}"),
            };
            let mut cmd = Command::new("dxc");
            cmd.args(["-T", &profile, "-E", &entry, "-Fo"])
                .arg(dir.join(format!("{name}_{entry}.dxil")))
                .arg(&path);
            if half {
                cmd.arg("-enable-16bit-types");
            }
            run(&format!("dxc on {name}::{entry}"), &mut cmd);
        }
    }
}

#[test]
fn msl_compiles_with_the_metal_compiler() {
    let t = tool(
        "metal",
        Command::new("xcrun").args(["-sdk", "macosx", "metal", "-v"]),
    );
    if !t.available {
        return;
    }
    let dir = out_dir();
    for (name, a) in corpus() {
        let path = dir.join(format!("{name}.metal"));
        std::fs::write(&path, &a.msl).unwrap();
        run(
            &format!("the Metal compiler on {name}"),
            Command::new("xcrun")
                .args(["-sdk", "macosx", "metal", "-std=macos-metal2.1", "-c"])
                .arg(&path)
                .arg("-o")
                .arg(dir.join(format!("{name}.air"))),
        );
    }
}

#[test]
fn glsl_compiles_with_glslang() {
    let t = tool("glslang", Command::new("glslangValidator").arg("--version"));
    if !t.available {
        return;
    }
    let dir = out_dir();
    for (name, a) in corpus() {
        for g in &a.glsl {
            let path = dir.join(format!("{name}_{}.glsl", g.entry_point));
            std::fs::write(&path, &g.source).unwrap();
            let stage = match g.stage {
                Stage::Vertex => "vert",
                Stage::Fragment => "frag",
                Stage::Compute => "comp",
            };
            run(
                &format!("glslang on {name}::{}", g.entry_point),
                Command::new("glslangValidator")
                    .args(["-S", stage])
                    .arg(&path),
            );
        }
    }
}

/// The module's entry points with their stages, from its Metal bindings.
fn entry_points(a: &Artifacts) -> Vec<(String, Stage)> {
    let module = naga::front::wgsl::parse_str(&a.wgsl).expect("WGSL parses");
    module
        .entry_points
        .iter()
        .map(|e| {
            let stage = match e.stage {
                naga::ShaderStage::Vertex => Stage::Vertex,
                naga::ShaderStage::Fragment => Stage::Fragment,
                _ => Stage::Compute,
            };
            (e.name.clone(), stage)
        })
        .collect()
}
