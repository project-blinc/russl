//! Programs read from files. A directory is a module tree: `dir/a/b.russl`, or
//! `dir/a/b/mod.russl`, is the module `crate::a::b`. A file's `use` items name the files it
//! imports, by a path that starts at `crate`, `super` or `self`, as in Rust.

use crate::{Artifacts, Constant, Diagnostic, Module};
use proc_macro2::{Span, TokenStream};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One file of a program, kept for rendering its diagnostics.
pub struct SourceFile {
    pub path: PathBuf,
    pub text: String,
    /// What `Span::file` reports for this file's tokens.
    key: Option<String>,
}

/// A root file and every file it reaches through `use`, each read and parsed once.
pub struct Program {
    /// `modules[0]` and `files[0]` are the root; the two lists share indices.
    pub modules: Vec<Module>,
    pub files: Vec<SourceFile>,
}

impl Program {
    /// Read `root` and what it imports, with `dir` the module tree's root. An error reading
    /// the root itself is the `Err`; every other problem is a diagnostic on the program, which
    /// then holds the files read so far.
    pub fn load(dir: &Path, root: &Path) -> Result<(Program, Vec<Diagnostic>), String> {
        let text = std::fs::read_to_string(root).map_err(|e| format!("{}: {e}", root.display()))?;
        let mut program = Program {
            modules: Vec::new(),
            files: Vec::new(),
        };
        let mut diagnostics = Vec::new();
        let path = match module_path(dir, root) {
            Ok(p) => p,
            Err(message) => return Err(format!("{}: {message}", root.display())),
        };
        let mut index: HashMap<Vec<String>, usize> = HashMap::new();
        let mut queue = vec![(program.add(root, text, &mut diagnostics), path.clone())];
        index.insert(path, 0);
        while let Some((m, path)) = queue.pop() {
            let Ok(file) = syn::parse2::<syn::File>(program.modules[m].source.clone()) else {
                // Reported when the program compiles.
                continue;
            };
            for (key, span) in crate::front::use_modules(&file) {
                let target = match resolve(&path, &key) {
                    Ok(t) => t,
                    Err(message) => {
                        diagnostics.push(Diagnostic::error(span, message));
                        continue;
                    }
                };
                if let Some(&i) = index.get(&target) {
                    program.modules[m].imports.push((key, i));
                    continue;
                }
                let Some((file, text)) = find(dir, &target) else {
                    // Relative to the tree, as a reader of the `use` item sees it.
                    let base: PathBuf = target.iter().collect();
                    diagnostics.push(Diagnostic::error(
                        span,
                        format!(
                            "no file for module `{key}`: expected {} or {} in {}",
                            base.with_extension("russl").display(),
                            base.join("mod.russl").display(),
                            dir.display()
                        ),
                    ));
                    continue;
                };
                let i = program.add(&file, text, &mut diagnostics);
                index.insert(target.clone(), i);
                program.modules[m].imports.push((key, i));
                queue.push((i, target));
            }
        }
        Ok((program, diagnostics))
    }

    pub fn compile(&self) -> Result<Artifacts, Vec<Diagnostic>> {
        crate::compile_program(&self.modules)
    }

    pub fn compile_with(&self, constants: &[Constant]) -> Result<Artifacts, Vec<Diagnostic>> {
        crate::compile_program_with(&self.modules, constants)
    }

    /// The file a diagnostic's span lies in; the root when the span names no file, as for
    /// errors about the whole program.
    pub fn file_of(&self, span: Span) -> &SourceFile {
        let key = span.file();
        self.files
            .iter()
            .find(|f| f.key.as_deref() == Some(key.as_str()))
            .unwrap_or(&self.files[0])
    }

    fn add(&mut self, path: &Path, text: String, diagnostics: &mut Vec<Diagnostic>) -> usize {
        let source = text.parse::<TokenStream>().unwrap_or_else(|e| {
            diagnostics.push(Diagnostic::error(e.span(), e.to_string()));
            TokenStream::new()
        });
        let key = source.clone().into_iter().next().map(|t| t.span().file());
        self.modules.push(Module {
            source,
            imports: Vec::new(),
        });
        self.files.push(SourceFile {
            path: path.to_path_buf(),
            text,
            key,
        });
        self.modules.len() - 1
    }
}

/// The module path of a file in the tree under `dir`.
fn module_path(dir: &Path, file: &Path) -> Result<Vec<String>, String> {
    let rel = file
        .strip_prefix(dir)
        .map_err(|_| format!("is not inside the module tree at {}", dir.display()))?;
    let mut path: Vec<String> = rel
        .with_extension("")
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if path.last().is_some_and(|l| l == "mod") {
        path.pop();
    }
    Ok(path)
}

/// The module a `use` path written in module `current` names.
fn resolve(current: &[String], key: &str) -> Result<Vec<String>, String> {
    let mut segments = key.split("::");
    let mut path = match segments.next() {
        Some("crate") => Vec::new(),
        Some("self") => current.to_vec(),
        Some("super") => {
            let mut p = current.to_vec();
            p.pop();
            p
        }
        _ => {
            return Err(format!(
                "`{key}` starts at `crate`, `super` or `self`: a file imports other files of its \
                 module tree"
            ));
        }
    };
    let mut leading = true;
    for s in segments {
        if s == "super" && leading {
            if path.pop().is_none() {
                return Err(format!("`{key}` goes above the module tree's root"));
            }
            continue;
        }
        leading = false;
        path.push(s.to_string());
    }
    if path.is_empty() {
        return Err(format!(
            "`{key}` names the module tree's root, which is not a file"
        ));
    }
    Ok(path)
}

/// The file of module `path`, and its text.
fn find(dir: &Path, path: &[String]) -> Option<(PathBuf, String)> {
    let base = dir.join(path.iter().collect::<PathBuf>());
    [base.with_extension("russl"), base.join("mod.russl")]
        .into_iter()
        .find_map(|p| std::fs::read_to_string(&p).ok().map(|t| (p, t)))
}

#[cfg(test)]
mod tests {
    use super::resolve;

    fn p(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn paths_resolve_as_rust_resolves_them() {
        let here = p(&["sdf", "core"]);
        assert_eq!(
            resolve(&here, "super::common").unwrap(),
            p(&["sdf", "common"])
        );
        assert_eq!(resolve(&here, "super::super::lib").unwrap(), p(&["lib"]));
        assert_eq!(resolve(&here, "crate::clip").unwrap(), p(&["clip"]));
        assert_eq!(
            resolve(&here, "self::parts").unwrap(),
            p(&["sdf", "core", "parts"])
        );
        assert!(resolve(&here, "common").is_err());
        assert!(resolve(&p(&["a"]), "super::super::x").is_err());
    }
}
