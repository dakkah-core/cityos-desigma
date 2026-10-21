//! Code blocks of a board: the node props, a small hand-written tokenizer for the common
//! languages and the dark / light colour themes. The renderer lowers a code block to a rounded
//! frame, a line-number gutter and one text node whose runs carry the token colours.

use crate::board::board_text;
use crate::{Color, Paint, TextAlignH, TextAlignV, TextAutoResize, TextProps, TextRun};
use serde::{Deserialize, Serialize};

/// Monospace family of code blocks (the open-source stand-in the font service knows for
/// Menlo, Consolas and friends).
pub const CODE_FONT: &str = "JetBrains Mono";
/// Default text size of a code block.
pub const CODE_FONT_SIZE: f32 = 16.0;
/// Default box of a new code block.
pub const CODE_BLOCK_W: f32 = 480.0;
pub const CODE_BLOCK_H: f32 = 240.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CodeLang {
    #[default]
    Plain,
    Rust,
    JavaScript,
    TypeScript,
    Python,
    Json,
    Html,
    Css,
}

impl CodeLang {
    pub const ALL: [CodeLang; 8] =
        [CodeLang::Plain, CodeLang::Rust, CodeLang::JavaScript, CodeLang::TypeScript, CodeLang::Python, CodeLang::Json, CodeLang::Html, CodeLang::Css];

    pub fn label(self) -> &'static str {
        match self {
            CodeLang::Plain => "Plain text",
            CodeLang::Rust => "Rust",
            CodeLang::JavaScript => "JavaScript",
            CodeLang::TypeScript => "TypeScript",
            CodeLang::Python => "Python",
            CodeLang::Json => "JSON",
            CodeLang::Html => "HTML",
            CodeLang::Css => "CSS",
        }
    }

    /// The language after this one in [`CodeLang::ALL`], wrapping around.
    pub fn next(self) -> CodeLang {
        let i = Self::ALL.iter().position(|l| *l == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CodeTheme {
    #[default]
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Plain,
    Keyword,
    String,
    Number,
    Comment,
    Function,
    Type,
    Tag,
    Attr,
}

/// A highlighted span (byte range) of the source; gaps between tokens are plain text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
}

impl CodeTheme {
    pub fn background(self) -> Color {
        rgb(match self {
            CodeTheme::Dark => 0x1E1E1E,
            CodeTheme::Light => 0xF6F8FA,
        })
    }

    pub fn foreground(self) -> Color {
        rgb(match self {
            CodeTheme::Dark => 0xD4D4D4,
            CodeTheme::Light => 0x24292F,
        })
    }

    /// Colour of the line numbers.
    pub fn gutter(self) -> Color {
        self.foreground().with_alpha(0.4)
    }

    pub fn token(self, kind: TokenKind) -> Color {
        let (dark, light) = match kind {
            TokenKind::Plain => (0xD4D4D4, 0x24292F),
            TokenKind::Keyword => (0xC586C0, 0xCF222E),
            TokenKind::String => (0xCE9178, 0x0A3069),
            TokenKind::Number => (0xB5CEA8, 0x0550AE),
            TokenKind::Comment => (0x6A9955, 0x6E7781),
            TokenKind::Function => (0xDCDCAA, 0x8250DF),
            TokenKind::Type => (0x4EC9B0, 0x953800),
            TokenKind::Tag => (0x569CD6, 0x116329),
            TokenKind::Attr => (0x9CDCFE, 0x0550AE),
        };
        rgb(if self == CodeTheme::Dark { dark } else { light })
    }
}

fn rgb(c: u32) -> Color {
    Color::rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// A code block: the code is the text, drawn in monospace with token colours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CodeBlockProps {
    /// The code; its style carries the font size (colours come from the theme).
    pub text: TextProps,
    pub lang: CodeLang,
    pub theme: CodeTheme,
    pub line_numbers: bool,
}

impl Default for CodeBlockProps {
    fn default() -> Self {
        let mut text = board_text(CODE_FONT_SIZE, TextAlignH::Left, TextAlignV::Top);
        text.style.font_family = CODE_FONT.into();
        text.style.font_weight = 400;
        text.style.fills = vec![Paint::solid(CodeTheme::Dark.foreground())];
        Self { text, lang: CodeLang::Plain, theme: CodeTheme::Dark, line_numbers: true }
    }
}

/// Where the parts of a code block sit (node-local).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CodeLayout {
    pub corner: f32,
    pub pad: f32,
    /// Line-number column, when shown.
    pub gutter: Option<crate::Rect>,
    pub text_box: crate::Rect,
}

impl CodeBlockProps {
    /// Number of lines of the code (an empty block has one).
    pub fn line_count(&self) -> usize {
        self.text.content.split('\n').count().max(1)
    }

    pub fn layout(&self, size: crate::Vec2) -> CodeLayout {
        let fs = self.text.style.font_size.max(1.0);
        let pad = (fs * 0.9).max(8.0);
        let digits = self.line_count().to_string().len().max(2) as f32;
        let gutter_w = if self.line_numbers { digits * fs * 0.62 } else { 0.0 };
        let h = (size.y - 2.0 * pad).max(1.0);
        let gutter = self.line_numbers.then(|| crate::Rect::new(pad, pad, gutter_w, h));
        let tx = pad + if self.line_numbers { gutter_w + pad * 0.8 } else { 0.0 };
        CodeLayout { corner: (fs * 0.6).max(4.0), pad, gutter, text_box: crate::Rect::new(tx, pad, (size.x - tx - pad).max(1.0), h) }
    }

    /// The code as text props for drawing and editing: no wrapping, the theme's foreground and a run per
    /// highlighted token. Runs stored in `text` are ignored (they are recomputed from the code).
    pub fn styled_text(&self) -> TextProps {
        let mut t = self.text.clone();
        t.style.font_family = CODE_FONT.into();
        t.style.fills = vec![Paint::solid(self.theme.foreground())];
        t.auto_resize = TextAutoResize::WidthAndHeight;
        t.runs.clear();
        for tok in tokenize(self.lang, &t.content) {
            let mut style = t.style.clone();
            style.fills = vec![Paint::solid(self.theme.token(tok.kind))];
            t.runs.push(TextRun { start: tok.start, end: tok.end, style });
        }
        t
    }

    /// The line numbers `1..=n`, one per line, as right-aligned text in the gutter colour.
    pub fn gutter_text(&self) -> TextProps {
        let mut t = self.styled_text();
        t.runs.clear();
        t.content = (1..=self.line_count()).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
        t.style.fills = vec![Paint::solid(self.theme.gutter())];
        t.style.align_h = TextAlignH::Right;
        t
    }
}

// ----------------------------------------------------------------- tokenizer

struct Spec {
    keywords: &'static [&'static str],
    /// Words drawn as types besides capitalised identifiers.
    types: &'static [&'static str],
    line_comments: &'static [&'static str],
    block_comments: bool,
    /// Quote characters that open a string ended by the same character.
    quotes: &'static [u8],
    /// `name!(` is a macro call (drawn as a function).
    macros: bool,
    /// `'a` lifetimes (Rust) are drawn as types and `'` is not a quote.
    lifetimes: bool,
    triple_quotes: bool,
}

const RUST: Spec = Spec {
    keywords: &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let",
        "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use",
        "where", "while",
    ],
    types: &["i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "bool", "char", "str"],
    line_comments: &["//"],
    block_comments: true,
    quotes: b"\"",
    macros: true,
    lifetimes: true,
    triple_quotes: false,
};

const JS: Spec = Spec {
    keywords: &[
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "default",
        "delete",
        "do",
        "else",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "from",
        "function",
        "if",
        "import",
        "in",
        "instanceof",
        "let",
        "new",
        "null",
        "of",
        "return",
        "static",
        "super",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "typeof",
        "undefined",
        "var",
        "void",
        "while",
        "yield",
    ],
    types: &[],
    line_comments: &["//"],
    block_comments: true,
    quotes: b"\"'`",
    macros: false,
    lifetimes: false,
    triple_quotes: false,
};

const TS: Spec = Spec {
    keywords: &[
        "abstract",
        "as",
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "declare",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "from",
        "function",
        "if",
        "implements",
        "import",
        "in",
        "instanceof",
        "interface",
        "let",
        "namespace",
        "new",
        "null",
        "of",
        "private",
        "protected",
        "public",
        "readonly",
        "return",
        "static",
        "super",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "type",
        "typeof",
        "undefined",
        "var",
        "void",
        "while",
        "yield",
    ],
    types: &["any", "boolean", "never", "number", "string", "unknown", "object", "symbol", "bigint"],
    line_comments: &["//"],
    block_comments: true,
    quotes: b"\"'`",
    macros: false,
    lifetimes: false,
    triple_quotes: false,
};

const PY: Spec = Spec {
    keywords: &[
        "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "finally",
        "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "self", "try", "while", "with",
        "yield",
    ],
    types: &["int", "str", "float", "bool", "list", "dict", "set", "tuple", "bytes"],
    line_comments: &["#"],
    block_comments: false,
    quotes: b"\"'",
    macros: false,
    lifetimes: false,
    triple_quotes: true,
};

const JSON: Spec = Spec {
    keywords: &["true", "false", "null"],
    types: &[],
    line_comments: &[],
    block_comments: false,
    quotes: b"\"",
    macros: false,
    lifetimes: false,
    triple_quotes: false,
};

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$'
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// Highlighted spans of `src` in `lang`, in order and not overlapping.
pub fn tokenize(lang: CodeLang, src: &str) -> Vec<Token> {
    match lang {
        CodeLang::Plain => Vec::new(),
        CodeLang::Rust => generic(&RUST, src),
        CodeLang::JavaScript => generic(&JS, src),
        CodeLang::TypeScript => generic(&TS, src),
        CodeLang::Python => generic(&PY, src),
        CodeLang::Json => json(src),
        CodeLang::Html => html(src),
        CodeLang::Css => css(src),
    }
}

/// End of the string that opens at `i` (quote `q`): after the closing quote, at a newline (for
/// quotes that cannot span lines) or at the end of the text.
fn string_end(b: &[u8], i: usize, q: u8, multiline: bool) -> usize {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            c if c == q => return j + 1,
            b'\n' if !multiline => return j,
            _ => j += 1,
        }
    }
    b.len()
}

fn number_end(b: &[u8], i: usize) -> usize {
    let mut j = i;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || (b[j] == b'.' && b.get(j + 1).is_some_and(|c| c.is_ascii_digit()))) {
        j += 1;
    }
    j
}

fn generic(spec: &Spec, src: &str) -> Vec<Token> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let push = |out: &mut Vec<Token>, start, end, kind| out.push(Token { start, end, kind });
    while i < b.len() {
        let c = b[i];
        if spec.line_comments.iter().any(|p| b[i..].starts_with(p.as_bytes())) {
            let end = b[i..].iter().position(|c| *c == b'\n').map_or(b.len(), |n| i + n);
            push(&mut out, i, end, TokenKind::Comment);
            i = end;
        } else if spec.block_comments && b[i..].starts_with(b"/*") {
            let end = src[i + 2..].find("*/").map_or(b.len(), |n| i + 2 + n + 2);
            push(&mut out, i, end, TokenKind::Comment);
            i = end;
        } else if spec.triple_quotes && (b[i..].starts_with(b"\"\"\"") || b[i..].starts_with(b"'''")) {
            let q = &src[i..i + 3];
            let end = src[i + 3..].find(q).map_or(b.len(), |n| i + 3 + n + 3);
            push(&mut out, i, end, TokenKind::String);
            i = end;
        } else if spec.lifetimes && c == b'\'' {
            // 'a lifetime, or a char literal such as 'x' or '\n'
            let is_char = b.get(i + 2) == Some(&b'\'') || b.get(i + 1) == Some(&b'\\');
            if is_char {
                let end = string_end(b, i, b'\'', false);
                push(&mut out, i, end, TokenKind::String);
                i = end;
            } else {
                let mut j = i + 1;
                while j < b.len() && is_ident(b[j]) {
                    j += 1;
                }
                push(&mut out, i, j.max(i + 1), TokenKind::Type);
                i = j.max(i + 1);
            }
        } else if spec.quotes.contains(&c) {
            let end = string_end(b, i, c, c == b'`');
            push(&mut out, i, end, TokenKind::String);
            i = end;
        } else if c.is_ascii_digit() {
            let end = number_end(b, i);
            push(&mut out, i, end, TokenKind::Number);
            i = end;
        } else if is_ident_start(c) {
            let mut j = i + 1;
            while j < b.len() && is_ident(b[j]) {
                j += 1;
            }
            let word = &src[i..j];
            let next = b[j..].iter().find(|c| **c != b' ' && **c != b'\t');
            let kind = if spec.keywords.contains(&word) {
                Some(TokenKind::Keyword)
            } else if spec.types.contains(&word) {
                Some(TokenKind::Type)
            } else if spec.macros && b.get(j) == Some(&b'!') && b.get(j + 1).is_some_and(|c| matches!(c, b'(' | b'[' | b'{')) {
                j += 1;
                Some(TokenKind::Function)
            } else if next == Some(&b'(') {
                Some(TokenKind::Function)
            } else if c.is_ascii_uppercase() && word.len() > 1 && word.bytes().any(|c| c.is_ascii_lowercase()) {
                Some(TokenKind::Type)
            } else {
                None
            };
            if let Some(k) = kind {
                push(&mut out, i, j, k);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

fn json(src: &str) -> Vec<Token> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                let end = string_end(b, i, b'"', false);
                let key = b[end..].iter().find(|c| !c.is_ascii_whitespace()) == Some(&b':');
                out.push(Token { start: i, end, kind: if key { TokenKind::Attr } else { TokenKind::String } });
                i = end;
            }
            c if c.is_ascii_digit() || (c == b'-' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit())) => {
                let end = number_end(b, i + 1);
                out.push(Token { start: i, end, kind: TokenKind::Number });
                i = end;
            }
            c if c.is_ascii_alphabetic() => {
                let mut j = i + 1;
                while j < b.len() && b[j].is_ascii_alphabetic() {
                    j += 1;
                }
                if JSON.keywords.contains(&&src[i..j]) {
                    out.push(Token { start: i, end: j, kind: TokenKind::Keyword });
                }
                i = j;
            }
            _ => i += 1,
        }
    }
    out
}

fn html(src: &str) -> Vec<Token> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(b"<!--") {
            let end = src[i + 4..].find("-->").map_or(b.len(), |n| i + 4 + n + 3);
            out.push(Token { start: i, end, kind: TokenKind::Comment });
            i = end;
        } else if b[i] == b'<' && b.get(i + 1).is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, b'/' | b'!')) {
            // `<name` or `</name`, then attributes up to `>`
            let mut j = i + 1;
            if matches!(b[j], b'/' | b'!') {
                j += 1;
            }
            while j < b.len() && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'-' | b':')) {
                j += 1;
            }
            out.push(Token { start: i, end: j, kind: TokenKind::Tag });
            while j < b.len() && b[j] != b'>' {
                match b[j] {
                    b'"' | b'\'' => {
                        let end = string_end(b, j, b[j], true);
                        out.push(Token { start: j, end, kind: TokenKind::String });
                        j = end;
                    }
                    c if c.is_ascii_alphabetic() => {
                        let s = j;
                        while j < b.len() && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'-' | b':' | b'_')) {
                            j += 1;
                        }
                        out.push(Token { start: s, end: j, kind: TokenKind::Attr });
                    }
                    _ => j += 1,
                }
            }
            if j < b.len() {
                out.push(Token { start: j, end: j + 1, kind: TokenKind::Tag });
                j += 1;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

fn css(src: &str) -> Vec<Token> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut depth = 0usize;
    while i < b.len() {
        let c = b[i];
        if b[i..].starts_with(b"/*") {
            let end = src[i + 2..].find("*/").map_or(b.len(), |n| i + 2 + n + 2);
            out.push(Token { start: i, end, kind: TokenKind::Comment });
            i = end;
        } else if c == b'"' || c == b'\'' {
            let end = string_end(b, i, c, false);
            out.push(Token { start: i, end, kind: TokenKind::String });
            i = end;
        } else if c == b'{' {
            depth += 1;
            i += 1;
        } else if c == b'}' {
            depth = depth.saturating_sub(1);
            i += 1;
        } else if c == b'@' {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-') {
                j += 1;
            }
            out.push(Token { start: i, end: j, kind: TokenKind::Keyword });
            i = j;
        } else if c == b'#' && depth > 0 {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_hexdigit() {
                j += 1;
            }
            out.push(Token { start: i, end: j, kind: TokenKind::Number });
            i = j.max(i + 1);
        } else if c.is_ascii_digit() || (c == b'.' && depth > 0 && b.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'.' || b[j] == b'%') {
                j += 1;
            }
            out.push(Token { start: i, end: j, kind: TokenKind::Number });
            i = j;
        } else if c.is_ascii_alphabetic() || c == b'-' || c == b'.' || c == b'#' {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'-' | b'_')) {
                j += 1;
            }
            if depth > 0 {
                // a property is the name before `:` inside a rule
                let after = b[j..].iter().find(|c| !c.is_ascii_whitespace());
                if after == Some(&b':') && c != b'.' {
                    out.push(Token { start: i, end: j, kind: TokenKind::Attr });
                }
            } else {
                out.push(Token { start: i, end: j, kind: if c == b'.' || c == b'#' { TokenKind::Type } else { TokenKind::Tag } });
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lang: CodeLang, src: &str) -> Vec<(&str, TokenKind)> {
        tokenize(lang, src).into_iter().map(|t| (&src[t.start..t.end], t.kind)).collect()
    }

    #[test]
    fn rust_tokens() {
        let k = kinds(CodeLang::Rust, "fn main() { let x: u32 = 42; // hi\n println!(\"a\"); }");
        assert!(k.contains(&("fn", TokenKind::Keyword)));
        assert!(k.contains(&("main", TokenKind::Function)));
        assert!(k.contains(&("u32", TokenKind::Type)));
        assert!(k.contains(&("42", TokenKind::Number)));
        assert!(k.contains(&("// hi", TokenKind::Comment)));
        assert!(k.contains(&("println!", TokenKind::Function)));
        assert!(k.contains(&("\"a\"", TokenKind::String)));
        let k = kinds(CodeLang::Rust, "fn f<'a>(c: char) { 'x'; }");
        assert!(k.contains(&("'a", TokenKind::Type)));
        assert!(k.contains(&("'x'", TokenKind::String)));
    }

    #[test]
    fn js_ts_python() {
        let k = kinds(CodeLang::JavaScript, "const s = `a ${b}`; /* c */ foo(1.5)");
        assert!(k.contains(&("const", TokenKind::Keyword)));
        assert!(k.contains(&("`a ${b}`", TokenKind::String)));
        assert!(k.contains(&("/* c */", TokenKind::Comment)));
        assert!(k.contains(&("foo", TokenKind::Function)));
        assert!(k.contains(&("1.5", TokenKind::Number)));
        let k = kinds(CodeLang::TypeScript, "let n: number = 1; interface Foo {}");
        assert!(k.contains(&("number", TokenKind::Type)));
        assert!(k.contains(&("interface", TokenKind::Keyword)));
        assert!(k.contains(&("Foo", TokenKind::Type)));
        let k = kinds(CodeLang::Python, "def f(x):  # c\n    return \"\"\"a\nb\"\"\"");
        assert!(k.contains(&("def", TokenKind::Keyword)));
        assert!(k.contains(&("f", TokenKind::Function)));
        assert!(k.contains(&("# c", TokenKind::Comment)));
        assert!(k.contains(&("\"\"\"a\nb\"\"\"", TokenKind::String)));
    }

    #[test]
    fn json_html_css() {
        let k = kinds(CodeLang::Json, "{\"a\": [1, -2.5, true, \"x\"]}");
        assert!(k.contains(&("\"a\"", TokenKind::Attr)));
        assert!(k.contains(&("-2.5", TokenKind::Number)));
        assert!(k.contains(&("true", TokenKind::Keyword)));
        assert!(k.contains(&("\"x\"", TokenKind::String)));
        let k = kinds(CodeLang::Html, "<!-- c --><a href=\"x\">t</a>");
        assert!(k.contains(&("<!-- c -->", TokenKind::Comment)));
        assert!(k.contains(&("<a", TokenKind::Tag)));
        assert!(k.contains(&("href", TokenKind::Attr)));
        assert!(k.contains(&("\"x\"", TokenKind::String)));
        assert!(k.contains(&("</a", TokenKind::Tag)));
        let k = kinds(CodeLang::Css, ".a { color: #fff; margin: 4px; } @media x {}");
        assert!(k.contains(&(".a", TokenKind::Type)));
        assert!(k.contains(&("color", TokenKind::Attr)));
        assert!(k.contains(&("#fff", TokenKind::Number)));
        assert!(k.contains(&("4px", TokenKind::Number)));
        assert!(k.contains(&("@media", TokenKind::Keyword)));
    }

    #[test]
    fn tokens_are_ordered_in_bounds_and_on_char_boundaries() {
        let src = "let s = \"héllo ✓\"; // ünï\nfn é() {}";
        for lang in CodeLang::ALL {
            let toks = tokenize(lang, src);
            let mut at = 0;
            for t in &toks {
                assert!(t.start >= at && t.end > t.start && t.end <= src.len(), "{lang:?} {t:?}");
                assert!(src.is_char_boundary(t.start) && src.is_char_boundary(t.end), "{lang:?} {t:?}");
                at = t.end;
            }
        }
        // unterminated constructs end at the end of the text
        assert_eq!(tokenize(CodeLang::Rust, "\"abc").last().map(|t| t.end), Some(4));
        assert!(tokenize(CodeLang::Plain, "fn x").is_empty());
    }

    #[test]
    fn styled_text_has_a_run_per_token_and_gutter_counts_lines() {
        let mut c = CodeBlockProps { lang: CodeLang::Rust, ..Default::default() };
        c.text.content = "fn a() {}\nlet x = 1;".into();
        let t = c.styled_text();
        assert_eq!(t.runs.len(), tokenize(CodeLang::Rust, &c.text.content).len());
        assert_eq!(t.style.font_family, CODE_FONT);
        assert_eq!(c.gutter_text().content, "1\n2");
        let kw = t.runs.iter().find(|r| &c.text.content[r.start..r.end] == "fn").unwrap();
        assert_eq!(kw.style.fills[0], Paint::solid(CodeTheme::Dark.token(TokenKind::Keyword)));
        let lay = c.layout(crate::Vec2::new(400.0, 200.0));
        assert!(lay.gutter.is_some() && lay.text_box.x > lay.gutter.unwrap().x);
        c.line_numbers = false;
        assert!(c.layout(crate::Vec2::new(400.0, 200.0)).gutter.is_none());
    }

    #[test]
    fn serde_defaults_fill_missing_fields() {
        let c: CodeBlockProps = serde_json::from_str("{}").unwrap();
        assert_eq!(c, CodeBlockProps::default());
        assert_eq!(CodeLang::Css.next(), CodeLang::Plain);
    }
}
