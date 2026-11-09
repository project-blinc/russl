//! Diagnostics and the source-span table.

use proc_macro2::Span;

/// An index into the module's [`Spans`] table. naga spans carry it, so a validation error
/// maps back to the source token.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SpanId(pub u32);

/// Every source span the front end recorded.
#[derive(Default)]
pub struct Spans(Vec<Span>);

impl Spans {
    pub fn add(&mut self, span: Span) -> SpanId {
        self.0.push(span);
        SpanId(self.0.len() as u32 - 1)
    }

    pub fn get(&self, id: SpanId) -> Span {
        self.0
            .get(id.0 as usize)
            .copied()
            .unwrap_or_else(Span::call_site)
    }

    /// The source span a naga span stands for.
    pub fn for_naga(&self, span: naga::Span) -> Option<Span> {
        let range = span.to_range()?;
        self.0.get(range.start).copied()
    }
}

/// The naga span that stands for a source span.
pub fn naga_span(id: SpanId) -> naga::Span {
    naga::Span::new(id.0, id.0 + 1)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// An error or warning at a source token.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub span: Span,
    pub message: String,
}

impl Diagnostic {
    pub fn error(span: Span, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            span,
            message: message.into(),
        }
    }

    pub fn warning(span: Span, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            span,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        let at = self.span.start();
        if at.line > 0 {
            write!(f, "{}:{}: {kind}: {}", at.line, at.column + 1, self.message)
        } else {
            write!(f, "{kind}: {}", self.message)
        }
    }
}

/// Diagnostics gathered while compiling one module.
#[derive(Default)]
pub struct Sink {
    pub items: Vec<Diagnostic>,
}

impl Sink {
    pub fn error(&mut self, span: Span, message: impl Into<String>) {
        self.items.push(Diagnostic::error(span, message));
    }

    pub fn warning(&mut self, span: Span, message: impl Into<String>) {
        self.items.push(Diagnostic::warning(span, message));
    }

    pub fn has_errors(&self) -> bool {
        self.items.iter().any(|d| d.severity == Severity::Error)
    }
}
