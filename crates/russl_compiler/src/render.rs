//! Diagnostics rendered with source snippets, by ariadne.

use crate::{Diagnostic, Severity};
use ariadne::{Color, Config, Label, Report, ReportKind, Source};

/// Render diagnostics against the source they came from. `file` names it in the output.
pub fn render(diagnostics: &[Diagnostic], file: &str, source: &str, color: bool) -> String {
    let mut out = Vec::new();
    for d in diagnostics {
        let (kind, tint) = match d.severity {
            Severity::Error => (ReportKind::Error, Color::Red),
            Severity::Warning => (ReportKind::Warning, Color::Yellow),
        };
        let start = offset(source, d.span.start());
        let end = offset(source, d.span.end())
            .max(start + 1)
            .min(source.len().max(start + 1));
        let report = Report::build(kind, (file, start..end))
            .with_config(Config::default().with_color(color))
            .with_message(&d.message)
            .with_label(
                Label::new((file, start..end))
                    .with_message(&d.message)
                    .with_color(tint),
            )
            .finish();
        let _ = report.write((file, Source::from(source)), &mut out);
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The byte offset of a line (from 1) and column (in characters, from 0).
fn offset(source: &str, at: proc_macro2::LineColumn) -> usize {
    let line_start: usize = source
        .split_inclusive('\n')
        .take(at.line.saturating_sub(1))
        .map(str::len)
        .sum();
    let line = &source[line_start.min(source.len())..];
    line_start
        + line
            .chars()
            .take(at.column)
            .map(char::len_utf8)
            .sum::<usize>()
}
