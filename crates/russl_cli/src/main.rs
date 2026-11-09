//! `russl check <file>` reports errors and warnings; `russl emit <file> [target]` prints what the
//! file compiles to (`wgsl` by default, or `msl`, `hlsl`, `glsl`, or `spirv` as binary). A file
//! compiles with every file its `use` items reach; the module tree is rooted at the file's
//! directory unless `--root` names another.

use russl_compiler::Program;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: russl check <file> [--root <dir>] [--deny-warnings]
       russl emit <file> [wgsl|msl|hlsl|glsl|spirv] [--root <dir>]

--deny-warnings  fail on any warning, as CI does";

struct Args {
    command: String,
    file: PathBuf,
    target: String,
    root: Option<PathBuf>,
    deny_warnings: bool,
}

fn parse(mut args: impl Iterator<Item = String>) -> Option<Args> {
    let command = args.next()?;
    if command != "check" && command != "emit" {
        return None;
    }
    let mut out = Args {
        command,
        file: PathBuf::new(),
        target: "wgsl".into(),
        root: None,
        deny_warnings: false,
    };
    let mut positional = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--root" => out.root = Some(args.next()?.into()),
            "--deny-warnings" => out.deny_warnings = true,
            _ if a.starts_with("--") => return None,
            _ => positional.push(a),
        }
    }
    match (out.command.as_str(), positional.as_slice()) {
        ("check", [f]) | ("emit", [f]) => out.file = f.into(),
        ("emit", [f, t]) => {
            out.file = f.into();
            out.target = t.clone();
        }
        _ => return None,
    }
    Some(out)
}

fn main() -> ExitCode {
    let Some(args) = parse(std::env::args().skip(1)) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let dir = args
        .root
        .clone()
        .or_else(|| args.file.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let color = std::io::stderr().is_terminal();
    let (program, diagnostics) = match Program::load(&dir, &args.file) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    if !diagnostics.is_empty() {
        eprint!(
            "{}",
            russl_compiler::render_program(&diagnostics, &program, color)
        );
        return ExitCode::FAILURE;
    }
    let a = match program.compile() {
        Ok(a) => a,
        Err(d) => {
            eprint!("{}", russl_compiler::render_program(&d, &program, color));
            return ExitCode::FAILURE;
        }
    };
    eprint!(
        "{}",
        russl_compiler::render_program(&a.warnings, &program, color)
    );
    if args.deny_warnings && !a.warnings.is_empty() {
        eprintln!("{} warnings, denied", a.warnings.len());
        return ExitCode::FAILURE;
    }
    if args.command == "emit" {
        let mut out = std::io::stdout().lock();
        let written = match args.target.as_str() {
            "wgsl" => out.write_all(a.wgsl.as_bytes()),
            "msl" => out.write_all(a.msl.as_bytes()),
            "hlsl" => out.write_all(a.hlsl.as_bytes()),
            "spirv" => {
                let bytes: Vec<u8> = a.spirv.iter().flat_map(|w| w.to_le_bytes()).collect();
                out.write_all(&bytes)
            }
            "glsl" => {
                if let Some(missing) = &a.glsl_missing {
                    eprintln!("no GLSL: {missing}");
                    return ExitCode::FAILURE;
                }
                a.glsl.iter().try_for_each(|g| {
                    writeln!(out, "// {} ({:?})\n{}", g.entry_point, g.stage, g.source)
                })
            }
            other => {
                eprintln!("unknown target `{other}`\n{USAGE}");
                return ExitCode::from(2);
            }
        };
        if let Err(e) = written {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
