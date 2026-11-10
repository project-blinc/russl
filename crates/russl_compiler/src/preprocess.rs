//! Conditional source: `#if`, `#elif`, `#else` and `#endif` over pipeline constants.
//!
//! The pass runs on tokens, before the source is parsed, so a condition can choose anything: an
//! item, a binding, a struct field, an entry point's parameter, a statement. Its names are the
//! program's `#[pipeline]` constants, at their defaults or at the values a specialisation gives,
//! and a library module sees the same values as the shader that imports it.
//!
//! ```text
//! #[pipeline] const TIER: u32 = 0;
//!
//! #if TIER == 2
//!     #[group(0)] #[binding(1)] static PRIM_DATA: Texture2D<f32>;
//! #else
//!     #[group(0)] #[binding(1)] static PRIMITIVES: Storage<[Primitive]>;
//! #endif
//! ```
//!
//! A condition is a bool or a comparison of numbers, joined with `!`, `&&`, `||` and brackets. It
//! ends where the next token cannot continue it, so the body follows it directly.

use crate::{ConstValue, Constant, Diagnostic};
use proc_macro2::{Delimiter, Group, Spacing, Span, TokenStream, TokenTree};
use std::collections::BTreeMap;

/// The values conditions read, by constant name.
pub(crate) type Defines = BTreeMap<String, ConstValue>;

/// A piece of source: tokens, or a chain of conditional bodies.
enum Node {
    Tokens(Vec<TokenTree>),
    Chain(Vec<Arm>),
}

/// One `#if` or `#elif` body with its condition, or the `#else` body with none.
struct Arm {
    condition: Option<Expr>,
    body: Vec<Node>,
}

enum Expr {
    Value(ConstValue, Span),
    Name(String, Span),
    Not(Box<Expr>, Span),
    Binary(Op, Box<Expr>, Box<Expr>, Span),
}

#[derive(Copy, Clone, PartialEq)]
enum Op {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

enum Value {
    Bool(bool),
    Number(f64),
}

fn error(span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(span, message.into())
}

// ----- Parsing the directives -----

/// The directive at `trees[at]`, if one starts there: `#if`, `#elif`, `#else` or `#endif`.
fn directive(trees: &[TokenTree], at: usize) -> Option<(&'static str, Span)> {
    let TokenTree::Punct(hash) = trees.get(at)? else {
        return None;
    };
    if hash.as_char() != '#' {
        return None;
    }
    let TokenTree::Ident(word) = trees.get(at + 1)? else {
        return None;
    };
    let name = match word.to_string().as_str() {
        "if" => "if",
        "elif" => "elif",
        "else" => "else",
        "endif" => "endif",
        _ => return None,
    };
    Some((name, hash.span()))
}

fn nodes(trees: &[TokenTree]) -> Result<Vec<Node>, Diagnostic> {
    let mut at = 0;
    let nodes = nodes_until(trees, &mut at, false)?;
    Ok(nodes)
}

/// Parse nodes from `at` to the end, or, inside a chain, to the next `#elif`, `#else` or `#endif`
/// of this level, which is left unread.
fn nodes_until(
    trees: &[TokenTree],
    at: &mut usize,
    in_chain: bool,
) -> Result<Vec<Node>, Diagnostic> {
    let mut out = Vec::new();
    let mut plain = Vec::new();
    while *at < trees.len() {
        match directive(trees, *at) {
            Some(("if", _)) => {
                if !plain.is_empty() {
                    out.push(Node::Tokens(std::mem::take(&mut plain)));
                }
                out.push(chain(trees, at)?);
            }
            Some((name, span)) => {
                if in_chain {
                    break;
                }
                return Err(error(span, format!("`#{name}` without a matching `#if`")));
            }
            None => {
                plain.push(trees[*at].clone());
                *at += 1;
            }
        }
    }
    if !plain.is_empty() {
        out.push(Node::Tokens(plain));
    }
    Ok(out)
}

/// A chain from its `#if` to its `#endif`.
fn chain(trees: &[TokenTree], at: &mut usize) -> Result<Node, Diagnostic> {
    let (_, start) = directive(trees, *at).expect("a directive");
    let mut arms = Vec::new();
    let mut seen_else = false;
    // `#if` and `#elif` take a condition; `#else` does not.
    let mut next = "if";
    loop {
        *at += 2;
        let condition = if next == "else" {
            seen_else = true;
            None
        } else {
            Some(condition(trees, at)?)
        };
        let body = nodes_until(trees, at, true)?;
        arms.push(Arm { condition, body });
        match directive(trees, *at) {
            Some(("endif", _)) => {
                *at += 2;
                return Ok(Node::Chain(arms));
            }
            Some((name @ ("elif" | "else"), span)) => {
                if seen_else {
                    return Err(error(span, format!("`#{name}` after `#else`")));
                }
                next = name;
            }
            _ => return Err(error(start, "this `#if` has no `#endif`")),
        }
    }
}

// ----- Conditions -----

fn condition(trees: &[TokenTree], at: &mut usize) -> Result<Expr, Diagnostic> {
    or(trees, at)
}

fn or(trees: &[TokenTree], at: &mut usize) -> Result<Expr, Diagnostic> {
    let mut left = and(trees, at)?;
    while let Some((Op::Or, len, span)) = operator(trees, *at) {
        *at += len;
        let right = and(trees, at)?;
        left = Expr::Binary(Op::Or, Box::new(left), Box::new(right), span);
    }
    Ok(left)
}

fn and(trees: &[TokenTree], at: &mut usize) -> Result<Expr, Diagnostic> {
    let mut left = comparison(trees, at)?;
    while let Some((Op::And, len, span)) = operator(trees, *at) {
        *at += len;
        let right = comparison(trees, at)?;
        left = Expr::Binary(Op::And, Box::new(left), Box::new(right), span);
    }
    Ok(left)
}

fn comparison(trees: &[TokenTree], at: &mut usize) -> Result<Expr, Diagnostic> {
    let left = unary(trees, at)?;
    match operator(trees, *at) {
        Some((op @ (Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge), len, span)) => {
            *at += len;
            let right = unary(trees, at)?;
            Ok(Expr::Binary(op, Box::new(left), Box::new(right), span))
        }
        _ => Ok(left),
    }
}

fn unary(trees: &[TokenTree], at: &mut usize) -> Result<Expr, Diagnostic> {
    let Some(tree) = trees.get(*at) else {
        return Err(error(
            trees
                .last()
                .map(|t| t.span())
                .unwrap_or_else(Span::call_site),
            "expected a condition",
        ));
    };
    match tree {
        // `!` alone is negation; `!=` is an operator and cannot start an operand.
        TokenTree::Punct(p) if p.as_char() == '!' && operator(trees, *at).is_none() => {
            *at += 1;
            let inner = unary(trees, at)?;
            Ok(Expr::Not(Box::new(inner), p.span()))
        }
        TokenTree::Punct(p) if p.as_char() == '-' => {
            *at += 1;
            let span = p.span();
            match unary(trees, at)? {
                Expr::Value(ConstValue::Int(n), s) => Ok(Expr::Value(ConstValue::Int(-n), s)),
                Expr::Value(ConstValue::Float(n), s) => Ok(Expr::Value(ConstValue::Float(-n), s)),
                _ => Err(error(span, "`-` takes a number")),
            }
        }
        TokenTree::Group(g) if g.delimiter() == Delimiter::Parenthesis => {
            *at += 1;
            let inner: Vec<TokenTree> = g.stream().into_iter().collect();
            let mut pos = 0;
            let expr = condition(&inner, &mut pos)?;
            if let Some(extra) = inner.get(pos) {
                return Err(error(extra.span(), "unexpected token in the condition"));
            }
            Ok(expr)
        }
        TokenTree::Ident(i) => {
            *at += 1;
            let span = i.span();
            Ok(match i.to_string().as_str() {
                "true" => Expr::Value(ConstValue::Bool(true), span),
                "false" => Expr::Value(ConstValue::Bool(false), span),
                name => Expr::Name(name.to_string(), span),
            })
        }
        TokenTree::Literal(l) => {
            *at += 1;
            let span = l.span();
            number(&l.to_string())
                .map(|v| Expr::Value(v, span))
                .ok_or_else(|| error(span, "expected a number, a bool or a pipeline constant"))
        }
        other => Err(error(
            other.span(),
            "expected a number, a bool or a pipeline constant",
        )),
    }
}

/// A number literal, with its type suffix left off.
fn number(text: &str) -> Option<ConstValue> {
    let digits = text
        .trim_end_matches(|c: char| c.is_ascii_alphabetic() && c != 'e' && c != 'E')
        .replace('_', "");
    if let Ok(n) = digits.parse::<i64>() {
        return Some(ConstValue::Int(n));
    }
    digits.parse::<f64>().ok().map(ConstValue::Float)
}

/// The operator at `trees[at]`, its length in tokens and its span.
fn operator(trees: &[TokenTree], at: usize) -> Option<(Op, usize, Span)> {
    let TokenTree::Punct(first) = trees.get(at)? else {
        return None;
    };
    let second = match trees.get(at + 1) {
        Some(TokenTree::Punct(p)) if first.spacing() == Spacing::Joint => Some(p.as_char()),
        _ => None,
    };
    let span = first.span();
    match (first.as_char(), second) {
        ('&', Some('&')) => Some((Op::And, 2, span)),
        ('|', Some('|')) => Some((Op::Or, 2, span)),
        ('=', Some('=')) => Some((Op::Eq, 2, span)),
        ('!', Some('=')) => Some((Op::Ne, 2, span)),
        ('<', Some('=')) => Some((Op::Le, 2, span)),
        ('>', Some('=')) => Some((Op::Ge, 2, span)),
        ('<', _) => Some((Op::Lt, 1, span)),
        ('>', _) => Some((Op::Gt, 1, span)),
        _ => None,
    }
}

fn evaluate(expr: &Expr, defines: &Defines) -> Result<Value, Diagnostic> {
    let value = |v: ConstValue| match v {
        ConstValue::Bool(b) => Value::Bool(b),
        ConstValue::Int(n) => Value::Number(n as f64),
        ConstValue::Float(n) => Value::Number(n),
    };
    match expr {
        Expr::Value(v, _) => Ok(value(*v)),
        Expr::Name(name, span) => defines.get(name).map(|v| value(*v)).ok_or_else(|| {
            error(
                *span,
                format!("`{name}` is not a pipeline constant with a literal value"),
            )
        }),
        Expr::Not(inner, span) => match evaluate(inner, defines)? {
            Value::Bool(b) => Ok(Value::Bool(!b)),
            Value::Number(_) => Err(error(*span, "`!` takes a bool")),
        },
        Expr::Binary(op, left, right, span) => {
            let (l, r) = (evaluate(left, defines)?, evaluate(right, defines)?);
            match (op, l, r) {
                (Op::And, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a && b)),
                (Op::Or, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a || b)),
                (Op::And | Op::Or, ..) => Err(error(*span, "`&&` and `||` take bools")),
                (Op::Eq, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a == b)),
                (Op::Ne, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a != b)),
                (_, Value::Number(a), Value::Number(b)) => Ok(Value::Bool(match op {
                    Op::Eq => a == b,
                    Op::Ne => a != b,
                    Op::Lt => a < b,
                    Op::Le => a <= b,
                    Op::Gt => a > b,
                    _ => a >= b,
                })),
                _ => Err(error(*span, "this compares a bool with a number")),
            }
        }
    }
}

fn truth(expr: &Expr, defines: &Defines) -> Result<bool, Diagnostic> {
    match evaluate(expr, defines)? {
        Value::Bool(b) => Ok(b),
        Value::Number(_) => Err(error(expr_span(expr), "a condition is a bool")),
    }
}

fn expr_span(expr: &Expr) -> Span {
    match expr {
        Expr::Value(_, s) | Expr::Name(_, s) | Expr::Not(_, s) | Expr::Binary(_, _, _, s) => *s,
    }
}

// ----- Output -----

/// How a chain is resolved: by the program's constants, or keeping every body, which is what
/// scanning source for its imports and attributes wants.
#[derive(Copy, Clone)]
enum Mode<'a> {
    Select(&'a Defines),
    All,
}

fn emit(nodes: &[Node], mode: Mode<'_>, out: &mut Vec<TokenTree>) -> Result<(), Diagnostic> {
    for node in nodes {
        match node {
            Node::Tokens(trees) => {
                for tree in trees {
                    out.push(match tree {
                        TokenTree::Group(g) => {
                            let inner = process(g.stream(), mode)?;
                            let mut group = Group::new(g.delimiter(), inner);
                            group.set_span(g.span());
                            TokenTree::Group(group)
                        }
                        other => other.clone(),
                    });
                }
            }
            Node::Chain(arms) => match mode {
                Mode::All => {
                    for arm in arms {
                        emit(&arm.body, mode, out)?;
                    }
                }
                Mode::Select(defines) => {
                    for arm in arms {
                        let taken = match &arm.condition {
                            Some(c) => truth(c, defines)?,
                            None => true,
                        };
                        if taken {
                            emit(&arm.body, mode, out)?;
                            break;
                        }
                    }
                }
            },
        }
    }
    Ok(())
}

fn process(stream: TokenStream, mode: Mode<'_>) -> Result<TokenStream, Diagnostic> {
    let trees: Vec<TokenTree> = stream.into_iter().collect();
    let parsed = nodes(&trees)?;
    let mut out = Vec::with_capacity(trees.len());
    emit(&parsed, mode, &mut out)?;
    Ok(out.into_iter().collect())
}

/// `source` with each chain reduced to the body its condition selects.
pub(crate) fn apply(source: TokenStream, defines: &Defines) -> Result<TokenStream, Diagnostic> {
    process(source, Mode::Select(defines))
}

/// `source` with every body kept and the directives removed.
pub fn all_branches(source: TokenStream) -> Result<TokenStream, Diagnostic> {
    process(source, Mode::All)
}

// ----- The values -----

/// Every module's `#[pipeline]` constants at their defaults, then the given values over them.
/// Declarations inside a conditional are not read: a constant a condition names is declared
/// outside it.
pub(crate) fn defines(
    sources: &[&TokenStream],
    values: &[Constant],
) -> Result<Defines, Diagnostic> {
    let mut defines = Defines::new();
    for source in sources {
        let trees: Vec<TokenTree> = (*source).clone().into_iter().collect();
        for node in nodes(&trees)? {
            let Node::Tokens(trees) = node else { continue };
            for (name, value, span) in pipeline_constants(&trees) {
                match defines.get(&name) {
                    Some(old) if *old != value => {
                        return Err(error(
                            span,
                            format!("the pipeline constant `{name}` has two different defaults"),
                        ));
                    }
                    _ => {
                        defines.insert(name, value);
                    }
                }
            }
        }
    }
    for c in values {
        defines.insert(c.name.clone(), c.value);
    }
    Ok(defines)
}

/// `#[pipeline] const NAME: T = literal;` items at this level of `trees`.
fn pipeline_constants(trees: &[TokenTree]) -> Vec<(String, ConstValue, Span)> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < trees.len() {
        let is_pipeline = matches!(&trees[i], TokenTree::Punct(p) if p.as_char() == '#')
            && matches!(trees.get(i + 1), Some(TokenTree::Group(g))
                if g.delimiter() == Delimiter::Bracket
                    && g.stream().to_string().trim() == "pipeline");
        if !is_pipeline {
            i += 1;
            continue;
        }
        i += 2;
        // Further attributes and `pub` may sit between the attribute and `const`.
        let mut j = i;
        while j < trees.len() && !matches!(&trees[j], TokenTree::Ident(w) if w == "const") {
            if matches!(&trees[j], TokenTree::Punct(p) if p.as_char() == ';') {
                break;
            }
            j += 1;
        }
        let Some(TokenTree::Ident(name)) = trees.get(j + 1) else {
            continue;
        };
        let span = name.span();
        let name = name.to_string();
        // The value is what follows `=` up to `;`.
        let Some(eq) = (j..trees.len())
            .find(|&k| matches!(&trees[k], TokenTree::Punct(p) if p.as_char() == '='))
        else {
            continue;
        };
        let end = (eq..trees.len())
            .find(|&k| matches!(&trees[k], TokenTree::Punct(p) if p.as_char() == ';'))
            .unwrap_or(trees.len());
        if let Some(value) = literal(&trees[eq + 1..end]) {
            found.push((name, value, span));
        }
        i = end;
    }
    found
}

fn literal(trees: &[TokenTree]) -> Option<ConstValue> {
    match trees {
        [TokenTree::Ident(i)] if i == "true" => Some(ConstValue::Bool(true)),
        [TokenTree::Ident(i)] if i == "false" => Some(ConstValue::Bool(false)),
        [TokenTree::Literal(l)] => number(&l.to_string()),
        [TokenTree::Punct(p), TokenTree::Literal(l)] if p.as_char() == '-' => {
            match number(&l.to_string())? {
                ConstValue::Int(n) => Some(ConstValue::Int(-n)),
                ConstValue::Float(n) => Some(ConstValue::Float(-n)),
                other => Some(other),
            }
        }
        _ => None,
    }
}
