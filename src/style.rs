//! Parse stylesheet module references without treating comments as imports.
use raffia::{
    ast::{
        AtRulePrelude, ImportPreludeHref, InterpolableStr, Statement, Str, Stylesheet, UrlValue,
    },
    Parser, Syntax,
};
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
pub struct StyleReference {
    pub specifier: String,
    /// Byte offset of the beginning of the quoted or unquoted token.
    pub offset: usize,
    pub kind: StyleKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StyleKind {
    Import,
    Use,
    Forward,
}

pub fn parse(path: &Path, source: &str) -> Result<Vec<StyleReference>, String> {
    let syntax = match path.extension().and_then(|s| s.to_str()) {
        Some("css") => Syntax::Css,
        Some("scss") => Syntax::Scss,
        Some("sass") => Syntax::Sass,
        Some("less") => Syntax::Less,
        _ => return Err("unsupported stylesheet extension".into()),
    };
    match parse_source(source, syntax) {
        Ok(references) => Ok(references),
        Err(original_error) if syntax == Syntax::Css => {
            let normalized = normalize_ie_star_properties(source);
            if normalized == source {
                Err(original_error)
            } else {
                parse_source(&normalized, syntax).map_err(|_| original_error)
            }
        }
        Err(original_error) if syntax == Syntax::Scss => {
            let normalized = normalize_custom_properties(source);
            if normalized == source {
                Err(original_error)
            } else {
                parse_source(&normalized, syntax).map_err(|_| original_error)
            }
        }
        Err(error) => Err(error),
    }
}

fn parse_source(source: &str, syntax: Syntax) -> Result<Vec<StyleReference>, String> {
    let mut parser = Parser::new(source, syntax);
    let ast = parser.parse::<Stylesheet>().map_err(|err| format!("{err:?}"))?;
    if !parser.recoverable_errors().is_empty() {
        return Err(format!("{:?}", parser.recoverable_errors()[0]));
    }
    let mut references = Vec::new();
    collect(&ast.statements, &mut references);
    Ok(references)
}

/// Raffia mishandles interpolation in strings inside custom property values.
/// Parse those declarations as ordinary properties on retry. Their values do
/// not contain stylesheet import rules; preserve all byte offsets and syntax.
fn normalize_custom_properties(source: &str) -> String {
    let mut bytes = source.as_bytes().to_vec();
    let mut index = 0;
    let mut previous = 0;
    let mut quote = None;
    let mut block_comment = false;
    let mut line_comment = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if block_comment {
            if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                block_comment = false;
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if line_comment {
            if byte == b'\n' {
                line_comment = false;
            }
            index += 1;
            continue;
        }
        if let Some(delimiter) = quote {
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == delimiter {
                quote = None;
            }
            index += 1;
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            block_comment = true;
            index += 2;
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            line_comment = true;
            index += 2;
            continue;
        }
        if byte == b'\'' || byte == b'"' {
            quote = Some(byte);
            previous = byte;
            index += 1;
            continue;
        }
        if byte == b'-' && bytes.get(index + 1) == Some(&b'-') && matches!(previous, b'{' | b';') {
            let mut end = index + 2;
            while bytes.get(end).is_some_and(|character| {
                character.is_ascii_alphanumeric() || matches!(*character, b'-' | b'_')
            }) {
                end += 1;
            }
            let mut colon = end;
            while bytes.get(colon).is_some_and(u8::is_ascii_whitespace) {
                colon += 1;
            }
            if end > index + 2 && bytes.get(colon) == Some(&b':') {
                bytes[index] = b'x';
                bytes[index + 1] = b'x';
            }
        }
        if !byte.is_ascii_whitespace() {
            previous = byte;
        }
        index += 1;
    }
    String::from_utf8(bytes).expect("replacing ASCII preserves UTF-8")
}

/// Old IE accepted `*property: value`. Replace the star with a space only in
/// declaration position and outside comments/strings, preserving byte offsets.
fn normalize_ie_star_properties(source: &str) -> String {
    let mut bytes = source.as_bytes().to_vec();
    let mut index = 0;
    let mut previous = 0;
    let mut quote = None;
    let mut comment = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if comment {
            if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                comment = false;
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if let Some(delimiter) = quote {
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == delimiter {
                quote = None;
            }
            index += 1;
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            comment = true;
            index += 2;
            continue;
        }
        if byte == b'\'' || byte == b'"' {
            quote = Some(byte);
            previous = byte;
            index += 1;
            continue;
        }
        if byte == b'*' && matches!(previous, b'{' | b';') {
            let start = index + 1;
            let mut end = start;
            while bytes.get(end).is_some_and(|character| {
                character.is_ascii_alphanumeric() || *character == b'-' || *character == b'_'
            }) {
                end += 1;
            }
            let mut colon = end;
            while bytes.get(colon).is_some_and(u8::is_ascii_whitespace) {
                colon += 1;
            }
            if end > start && bytes.get(colon) == Some(&b':') {
                bytes[index] = b' ';
            }
        }
        if !byte.is_ascii_whitespace() {
            previous = byte;
        }
        index += 1;
    }
    String::from_utf8(bytes).expect("replacing ASCII preserves UTF-8")
}

fn collect(statements: &[Statement<'_>], out: &mut Vec<StyleReference>) {
    for statement in statements {
        match statement {
            Statement::AtRule(rule) => {
                if let Some(prelude) = &rule.prelude {
                    match prelude {
                        AtRulePrelude::Import(import) => {
                            add_href(&import.href, StyleKind::Import, out);
                        }
                        AtRulePrelude::LessImport(import) => {
                            add_href(&import.href, StyleKind::Import, out);
                        }
                        AtRulePrelude::SassImport(import) => {
                            for path in &import.paths {
                                add_str(path, StyleKind::Import, out);
                            }
                        }
                        AtRulePrelude::SassUse(import) => {
                            add_interpolable(&import.path, StyleKind::Use, out);
                        }
                        AtRulePrelude::SassForward(import) => {
                            add_interpolable(&import.path, StyleKind::Forward, out);
                        }
                        _ => {}
                    }
                }
                if let Some(block) = &rule.block {
                    collect(&block.statements, out);
                }
            }
            Statement::QualifiedRule(rule) => collect(&rule.block.statements, out),
            Statement::SassIfAtRule(rule) => {
                collect(&rule.if_clause.block.statements, out);
                for clause in &rule.else_if_clauses {
                    collect(&clause.block.statements, out);
                }
                if let Some(block) = &rule.else_clause {
                    collect(&block.statements, out);
                }
            }
            _ => {}
        }
    }
}

fn add_href(href: &ImportPreludeHref<'_>, kind: StyleKind, out: &mut Vec<StyleReference>) {
    match href {
        ImportPreludeHref::Str(value) => add_interpolable(value, kind, out),
        ImportPreludeHref::Url(url) => match &url.value {
            Some(UrlValue::Raw(value)) => add(&value.value, value.span.start, kind, out),
            Some(UrlValue::Str(value)) => add_interpolable(value, kind, out),
            _ => {}
        },
    }
}
fn add_interpolable(value: &InterpolableStr<'_>, kind: StyleKind, out: &mut Vec<StyleReference>) {
    if let InterpolableStr::Literal(value) = value {
        add_str(value, kind, out);
    }
}
fn add_str(value: &Str<'_>, kind: StyleKind, out: &mut Vec<StyleReference>) {
    add(&value.value, value.span.start, kind, out);
}
fn add(value: &str, offset: usize, kind: StyleKind, out: &mut Vec<StyleReference>) {
    // Protocol URLs, network paths and Sass builtins resolve outside npm.
    // Relative paths remain: the reachability walker resolves those first.
    if value.is_empty()
        || value.starts_with(['/', '#'])
        || (value.contains(':') && !value.starts_with("pkg:"))
    {
        return;
    }
    out.push(StyleReference { specifier: value.into(), offset, kind });
}

#[cfg(test)]
mod tests;
