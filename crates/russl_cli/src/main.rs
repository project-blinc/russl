//! `russl check <file>` reports errors; `russl emit <file>` prints the WGSL.

use std::process::ExitCode;

const USAGE: &str = "usage: russl <check|emit> <file.rs>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (command, path) = match args.as_slice() {
        [c, p] if c == "check" || c == "emit" => (c.as_str(), p.as_str()),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let compiled = std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|source| {
            source
                .parse::<proc_macro2::TokenStream>()
                .map_err(|e| e.to_string())
        })
        .and_then(|tokens| russl_compiler::compile(tokens).map_err(|e| e.to_string()));
    match compiled {
        Ok(artifacts) => {
            if command == "emit" {
                print!("{}", artifacts.wgsl);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{path}: {e}");
            ExitCode::FAILURE
        }
    }
}
