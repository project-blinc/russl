//! `russl check <file>` reports errors and warnings; `russl emit <file> [target]` prints what the
//! file compiles to (`wgsl` by default, or `msl`, `hlsl`, `glsl`).

use std::io::IsTerminal;
use std::process::ExitCode;

const USAGE: &str =
    "usage: russl check <file.rs>\n       russl emit <file.rs> [wgsl|msl|hlsl|glsl]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (command, path, target) = match args.as_slice() {
        [c, p] if c == "check" || c == "emit" => (c.as_str(), p.as_str(), "wgsl"),
        [c, p, t] if c == "emit" => (c.as_str(), p.as_str(), t.as_str()),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let tokens = match source.parse::<proc_macro2::TokenStream>() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let color = std::io::stderr().is_terminal();
    match russl_compiler::compile(tokens) {
        Ok(a) => {
            eprint!(
                "{}",
                russl_compiler::render(&a.warnings, path, &source, color)
            );
            if command == "emit" {
                match target {
                    "wgsl" => print!("{}", a.wgsl),
                    "msl" => print!("{}", a.msl),
                    "hlsl" => print!("{}", a.hlsl),
                    "glsl" => {
                        for g in &a.glsl {
                            println!("// {} ({:?})\n{}", g.entry_point, g.stage, g.source);
                        }
                    }
                    other => {
                        eprintln!("unknown target `{other}`\n{USAGE}");
                        return ExitCode::from(2);
                    }
                }
            }
            ExitCode::SUCCESS
        }
        Err(diagnostics) => {
            eprint!(
                "{}",
                russl_compiler::render(&diagnostics, path, &source, color)
            );
            ExitCode::FAILURE
        }
    }
}
