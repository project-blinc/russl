//! Highlighters for russl and the languages it writes: WGSL, MSL, HLSL and GLSL, as Blinc's code
//! editor and viewer draw them. They share one rule shape (comments, attributes, preprocessor
//! lines, keywords, types, numbers, calls) and differ in their keyword and type lists.

use blinc_core::Color;
use blinc_layout::prelude::{SyntaxHighlighter, TokenRule, TokenType};
use blinc_theme::{ColorToken, ThemeState};
use std::sync::{Arc, OnceLock};

/// A shading language's syntax highlighter. The compiled rules are built once per language and
/// shared by every instance.
pub struct ShaderHighlighter {
    rules: Arc<[TokenRule]>,
}

#[derive(Clone, Copy)]
enum Lang {
    Russl,
    Wgsl,
    Msl,
    Hlsl,
    Glsl,
}

impl ShaderHighlighter {
    /// russl: Rust syntax with russl's types and attributes.
    pub fn russl() -> Self {
        Self::of(Lang::Russl)
    }

    /// WGSL, for WebGPU.
    pub fn wgsl() -> Self {
        Self::of(Lang::Wgsl)
    }

    /// Metal Shading Language.
    pub fn msl() -> Self {
        Self::of(Lang::Msl)
    }

    /// HLSL, for Direct3D.
    pub fn hlsl() -> Self {
        Self::of(Lang::Hlsl)
    }

    /// GLSL, including GLSL ES.
    pub fn glsl() -> Self {
        Self::of(Lang::Glsl)
    }

    fn of(lang: Lang) -> Self {
        static CACHE: [OnceLock<Arc<[TokenRule]>>; 5] = [
            OnceLock::new(),
            OnceLock::new(),
            OnceLock::new(),
            OnceLock::new(),
            OnceLock::new(),
        ];
        let rules = CACHE[lang as usize].get_or_init(|| rules(lang)).clone();
        Self { rules }
    }
}

impl SyntaxHighlighter for ShaderHighlighter {
    fn token_rules(&self) -> &[TokenRule] {
        &self.rules
    }

    // The fill, plain text and line numbers follow the theme; only tokens take the palette.
    fn background_color(&self) -> Color {
        ThemeState::get().color(ColorToken::Surface)
    }

    fn default_color(&self) -> Color {
        ThemeState::get().color(ColorToken::TextPrimary)
    }

    fn line_number_color(&self) -> Color {
        ThemeState::get().color(ColorToken::TextTertiary)
    }
}

fn rules(lang: Lang) -> Arc<[TokenRule]> {
    // The palette of Blinc's `RustHighlighter`, so shader and host code read alike.
    let keyword = Color::rgba(0.77, 0.56, 0.82, 1.0);
    let comment = Color::rgba(0.42, 0.54, 0.35, 1.0);
    let number = Color::rgba(0.71, 0.82, 0.57, 1.0);
    let function = Color::rgba(0.86, 0.82, 0.65, 1.0);
    let ty = Color::rgba(0.31, 0.76, 0.77, 1.0);
    let attribute = Color::rgba(0.61, 0.66, 0.86, 1.0);

    let (keywords, types): (&str, &str) = match lang {
        Lang::Russl => (
            "fn|let|mut|const|static|pub|use|struct|for|in|while|loop|if|else|match|return|break|continue|as|super|crate|self|extern|true|false",
            "bool|i32|u32|f32|f16|[UIBH]?Vec[234]|Mat[234]|Uniform|Storage|StorageMut|Workgroup|Atomic|Sampler|SamplerComparison|Texture[0-9A-Za-z]*",
        ),
        Lang::Wgsl => (
            "fn|let|var|const|override|struct|alias|if|else|switch|case|default|for|loop|while|break|continue|continuing|return|discard|enable|requires|diagnostic|true|false|uniform|storage|read|write|read_write|function|private|workgroup",
            "bool|i32|u32|f32|f16|vec[234][fiuh]?|mat[234]x[234][fh]?|array|atomic|ptr|sampler|sampler_comparison|texture_[a-z_0-9]+",
        ),
        Lang::Msl => (
            "using|namespace|struct|constant|device|thread|threadgroup|const|constexpr|static|inline|return|if|else|switch|case|default|for|while|do|break|continue|discard_fragment|vertex|fragment|kernel|template|typename|true|false",
            "bool|char|short|int|uint|half|float|void|(?:bool|char|short|int|uint|half|float)[234]|(?:half|float)[234]x[234]|texture[0-9a-z_]*|sampler|array|atomic_[a-z]+|metal::[a-z_0-9]+",
        ),
        Lang::Hlsl => (
            "struct|cbuffer|tbuffer|register|static|const|uniform|in|out|inout|return|if|else|switch|case|default|for|while|do|break|continue|discard|typedef|true|false|groupshared|nointerpolation",
            "bool|int|uint|half|float|double|void|(?:bool|int|uint|half|float|double)[1234]|(?:half|float|double)[1234]x[1234]|matrix|vector|Texture[0-9A-Za-z]*|RWTexture[0-9A-Za-z]*|SamplerState|SamplerComparisonState|ByteAddressBuffer|RWByteAddressBuffer|StructuredBuffer|RWStructuredBuffer",
        ),
        Lang::Glsl => (
            "struct|uniform|in|out|inout|layout|const|flat|smooth|highp|mediump|lowp|precision|return|if|else|switch|case|default|for|while|do|break|continue|discard|true|false|buffer|shared",
            "bool|int|uint|float|double|void|[biud]?vec[234]|mat[234](?:x[234])?|sampler[0-9A-Za-z]*|isampler[0-9A-Za-z]*|usampler[0-9A-Za-z]*|image[0-9A-Za-z]*",
        ),
    };

    let mut out = vec![
        TokenRule::new(r"//.*$", comment).token_type(TokenType::Comment),
        TokenRule::new(r"/\*.*?\*/", comment).token_type(TokenType::Comment),
    ];
    match lang {
        Lang::Russl => {
            out.push(
                TokenRule::new(r"#!?\[[^\]]*\]", attribute)
                    .token_type(TokenType::custom("attribute")),
            );
            out.push(TokenRule::new(r"\b[a-z_]+!", attribute).token_type(TokenType::Macro));
        }
        Lang::Wgsl => out.push(
            TokenRule::new(r"@[a-z_]+", attribute).token_type(TokenType::custom("attribute")),
        ),
        Lang::Msl => {
            out.push(
                TokenRule::new(r"\[\[[^\]]*\]\]", attribute)
                    .token_type(TokenType::custom("attribute")),
            );
            out.push(TokenRule::new(r"^\s*#.*$", attribute).token_type(TokenType::Macro));
        }
        Lang::Hlsl | Lang::Glsl => {
            out.push(TokenRule::new(r"^\s*#.*$", attribute).token_type(TokenType::Macro));
            out.push(
                TokenRule::new(r"\bSV_[A-Za-z]+\b", attribute)
                    .token_type(TokenType::custom("semantic")),
            );
        }
    }
    out.push(
        TokenRule::new(&format!(r"\b(?:{keywords})\b"), keyword)
            .bold()
            .token_type(TokenType::Keyword),
    );
    out.push(TokenRule::new(&format!(r"\b(?:{types})\b"), ty).token_type(TokenType::Type));
    out.push(TokenRule::new(r"\b[A-Z][a-zA-Z0-9_]*\b", ty).token_type(TokenType::Type));
    out.push(
        TokenRule::new(r"\b\d+(\.\d+)?([eE][+-]?\d+)?[fuhi]?\b", number)
            .token_type(TokenType::Number),
    );
    out.push(TokenRule::new(r"\b0x[0-9a-fA-F]+[ul]?\b", number).token_type(TokenType::Number));
    out.push(
        TokenRule::new(r"\b([a-z_][a-zA-Z0-9_]*)\s*\(", function).token_type(TokenType::Function),
    );
    Arc::from(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use blinc_layout::prelude::SyntaxHighlighter;

    /// The colour the highlighter gives the first occurrence of `word` in `line`.
    fn color_of(h: &ShaderHighlighter, line: &str, word: &str) -> Color {
        let styled = h.highlight(line);
        let at = line.find(word).expect("the word is in the line");
        let span = styled.lines[0]
            .spans
            .iter()
            .find(|s| s.start <= at && at < s.end)
            .expect("a span covers it");
        span.color
    }

    #[test]
    fn each_language_marks_its_own_keywords_types_and_attributes() {
        let wgsl = ShaderHighlighter::wgsl();
        let line = "@fragment fn fs_main() -> @location(0) vec4<f32> { return vec4(1.0); }";
        assert_eq!(
            color_of(&wgsl, line, "@fragment"),
            color_of(&wgsl, line, "@location")
        );
        assert_ne!(color_of(&wgsl, line, "fn"), color_of(&wgsl, line, "vec4"));

        let msl = ShaderHighlighter::msl();
        let line = "fragment float4 fs_main(constant Globals& g [[buffer(0)]]) {";
        assert_eq!(
            color_of(&msl, line, "float4"),
            color_of(&msl, line, "Globals")
        );
        assert_ne!(
            color_of(&msl, line, "[[buffer(0)]]"),
            color_of(&msl, line, "float4")
        );

        let hlsl = ShaderHighlighter::hlsl();
        let line = "cbuffer G : register(b0) { float4x4 m; };";
        assert_ne!(
            color_of(&hlsl, line, "cbuffer"),
            color_of(&hlsl, line, "float4x4")
        );

        let russl = ShaderHighlighter::russl();
        let line = "#[fragment] fn fs(p: Vec4) -> Vec4 { discard!(); TOY.time }";
        assert_eq!(
            color_of(&russl, line, "#[fragment]"),
            color_of(&russl, line, "discard!")
        );
        assert_ne!(color_of(&russl, line, "fn"), color_of(&russl, line, "Vec4"));

        let glsl = ShaderHighlighter::glsl();
        let line = "uniform highp sampler2D atlas; // the atlas";
        assert_ne!(
            color_of(&glsl, line, "sampler2D"),
            color_of(&glsl, line, "the atlas")
        );
    }
}
