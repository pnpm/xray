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
    collect(&ast.statements, syntax, &mut references);
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

/// How the tool processing a stylesheet treats the path an import names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lookup {
    /// The preprocessor loads the file, so a bare path can name a package.
    Load,
    /// Plain CSS imports (including preprocessor pass-through rules) are URLs
    /// relative to the stylesheet. Only the explicit bundler `~` prefix names
    /// a package here; a bare slash is not proof of a package requirement.
    PassThrough,
}

fn collect(statements: &[Statement<'_>], syntax: Syntax, out: &mut Vec<StyleReference>) {
    for statement in statements {
        match statement {
            Statement::AtRule(rule) => {
                if let Some(prelude) = &rule.prelude {
                    match prelude {
                        AtRulePrelude::Import(import) => {
                            let lookup = match syntax {
                                Syntax::Css => Lookup::PassThrough,
                                Syntax::Less => less_lookup(&import.href, &[]),
                                Syntax::Scss | Syntax::Sass => {
                                    let plain_css =
                                        matches!(import.href, ImportPreludeHref::Url(_))
                                            || import.media.is_some()
                                            || import.supports.is_some()
                                            || import.layer.is_some()
                                            || href_value(&import.href)
                                                .is_some_and(has_css_extension);
                                    if plain_css {
                                        Lookup::PassThrough
                                    } else {
                                        Lookup::Load
                                    }
                                }
                            };
                            add_href(&import.href, StyleKind::Import, lookup, out);
                        }
                        AtRulePrelude::LessImport(import) => {
                            let options: Vec<String> = import
                                .options
                                .names
                                .iter()
                                .map(|name| name.name.to_ascii_lowercase())
                                .collect();
                            let lookup = less_lookup(&import.href, &options);
                            add_href(&import.href, StyleKind::Import, lookup, out);
                        }
                        AtRulePrelude::SassImport(import) => {
                            for path in &import.paths {
                                let lookup = if has_css_extension(&path.value) {
                                    Lookup::PassThrough
                                } else {
                                    Lookup::Load
                                };
                                add_str(path, StyleKind::Import, lookup, out);
                            }
                        }
                        AtRulePrelude::SassUse(import) => {
                            add_interpolable(&import.path, StyleKind::Use, Lookup::Load, out);
                        }
                        AtRulePrelude::SassForward(import) => {
                            add_interpolable(&import.path, StyleKind::Forward, Lookup::Load, out);
                        }
                        _ => {}
                    }
                }
                if let Some(block) = &rule.block {
                    collect(&block.statements, syntax, out);
                }
            }
            Statement::QualifiedRule(rule) => collect(&rule.block.statements, syntax, out),
            Statement::SassIfAtRule(rule) => {
                collect(&rule.if_clause.block.statements, syntax, out);
                for clause in &rule.else_if_clauses {
                    collect(&clause.block.statements, syntax, out);
                }
                if let Some(block) = &rule.else_clause {
                    collect(&block.statements, syntax, out);
                }
            }
            _ => {}
        }
    }
}

/// Less loads an import unless it is told the file is CSS, by the `(css)`
/// option or by a `.css` extension that `(less)` or `(inline)` do not
/// override. An `(optional)` import builds without its file.
fn less_lookup(href: &ImportPreludeHref<'_>, options: &[String]) -> Lookup {
    let has = |option: &str| options.iter().any(|name| name == option);
    if has("optional") || has("css") {
        return Lookup::PassThrough;
    }
    if has("less") || has("inline") {
        return Lookup::Load;
    }
    let css_extension = href_value(href).is_some_and(|value| {
        let path = value.split(['?', '#']).next().unwrap_or(value);
        has_css_extension(path)
    });
    if css_extension {
        Lookup::PassThrough
    } else {
        Lookup::Load
    }
}

fn has_css_extension(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|extension| extension.eq_ignore_ascii_case("css"))
}

fn href_value<'a>(href: &'a ImportPreludeHref<'_>) -> Option<&'a str> {
    let value = match href {
        ImportPreludeHref::Str(value) => value,
        ImportPreludeHref::Url(url) => match &url.value {
            Some(UrlValue::Raw(value)) => return Some(&value.value),
            Some(UrlValue::Str(value)) => value,
            _ => return None,
        },
    };
    match value {
        InterpolableStr::Literal(value) => Some(&value.value),
        InterpolableStr::SassInterpolated(_) | InterpolableStr::LessInterpolated(_) => None,
    }
}

fn add_href(
    href: &ImportPreludeHref<'_>,
    kind: StyleKind,
    lookup: Lookup,
    out: &mut Vec<StyleReference>,
) {
    match href {
        ImportPreludeHref::Str(value) => add_interpolable(value, kind, lookup, out),
        ImportPreludeHref::Url(url) => match &url.value {
            Some(UrlValue::Raw(value)) => add(&value.value, value.span.start, kind, lookup, out),
            Some(UrlValue::Str(value)) => add_interpolable(value, kind, lookup, out),
            _ => {}
        },
    }
}
fn add_interpolable(
    value: &InterpolableStr<'_>,
    kind: StyleKind,
    lookup: Lookup,
    out: &mut Vec<StyleReference>,
) {
    if let InterpolableStr::Literal(value) = value {
        add_str(value, kind, lookup, out);
    }
}
fn add_str(value: &Str<'_>, kind: StyleKind, lookup: Lookup, out: &mut Vec<StyleReference>) {
    add(&value.value, value.span.start, kind, lookup, out);
}
fn add(value: &str, offset: usize, kind: StyleKind, lookup: Lookup, out: &mut Vec<StyleReference>) {
    // Protocol URLs, network paths and Sass builtins resolve outside npm.
    // Relative paths remain: the reachability walker resolves those first.
    if value.is_empty()
        || value.starts_with(['/', '#'])
        || (value.contains(':') && !value.starts_with("pkg:"))
    {
        return;
    }
    let as_written = value.starts_with(['.', '~'])
        || match lookup {
            Lookup::Load => true,
            Lookup::PassThrough => false,
        };
    // A path that names no package is still followed to a local file, so it
    // is kept, spelled the way a URL relative to the stylesheet resolves.
    let specifier = if as_written { value.to_string() } else { format!("./{value}") };
    out.push(StyleReference { specifier, offset, kind });
}

#[cfg(test)]
mod tests;
