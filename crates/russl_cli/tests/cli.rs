//! The `russl` command on a program of two files.

use std::path::PathBuf;
use std::process::Command;

fn tree(name: &str, main: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("russl_cli_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::write(
        dir.join("lib/sdf.russl"),
        "pub fn sd_circle(p: Vec2, r: f32) -> f32 { p.length() - r }\n",
    )
    .unwrap();
    std::fs::write(dir.join("main.russl"), main).unwrap();
    dir
}

fn russl(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_russl"))
        .args(args)
        .output()
        .unwrap()
}

const MAIN: &str = "use crate::lib::sdf::sd_circle;\n\
#[fragment]\n#[location(0)]\nfn fs(#[builtin(position)] p: Vec4) -> Vec4 {\n    Vec4::splat(sd_circle(p.xy, 4.0))\n}\n";

#[test]
fn check_and_emit_a_program_of_files() {
    let dir = tree("ok", MAIN);
    let main = dir.join("main.russl");
    let main = main.to_str().unwrap();
    let out = russl(&["check", main, "--deny-warnings"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = russl(&["emit", main]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("fn sd_circle"));
    let out = russl(&["emit", main, "spirv"]);
    assert_eq!(out.stdout[..4], 0x0723_0203u32.to_le_bytes());
}

#[test]
fn deny_warnings_fails_on_a_warning() {
    let main = MAIN.replace("    Vec4::splat", "    let unused = 1.0;\n    Vec4::splat");
    let dir = tree("warn", &main);
    let main = dir.join("main.russl");
    let main = main.to_str().unwrap();
    assert!(russl(&["check", main]).status.success());
    let out = russl(&["check", main, "--deny-warnings"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unused variable: `unused`"));
}

#[test]
fn root_moves_the_module_tree() {
    let dir = tree("root", "");
    std::fs::create_dir_all(dir.join("app")).unwrap();
    std::fs::write(dir.join("app/main.russl"), MAIN).unwrap();
    let main = dir.join("app/main.russl");
    let (main, root) = (main.to_str().unwrap(), dir.to_str().unwrap());
    assert!(
        !russl(&["check", main]).status.success(),
        "lib is not under app/"
    );
    let out = russl(&["check", main, "--root", root]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
