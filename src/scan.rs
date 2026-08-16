use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, CallExpression, ExportAllDeclaration, ExportFromDeclaration, Expression,
    ImportDeclaration, ImportExpression, TSExternalModuleReference, TSImportType,
};
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_span::SourceType;
use std::{collections::BTreeSet, path::Path};

/// Every module specifier a file asks for.
///
/// In a declaration file that includes positions which never execute —
/// `import type`, `import("pkg")` inside a type, `/// <reference types="pkg" />`
/// — and in executable code it includes `require("pkg")`, which is where most of
/// the ecosystem's undeclared dependencies actually live.
///
/// Returns `Err` when the file could not be parsed. Callers must report that
/// rather than treat it as a file with nothing to find.
pub fn specifiers(source: &str, path: &Path) -> Result<BTreeSet<Requirement>, String> {
    let allocator = Allocator::default();
    let parsed = parse(&allocator, source, path)?;

    let mut collector = Collector::default();
    collector.visit_program(&parsed);
    let mut found = collector.found;
    if is_declaration(path) {
        found.extend(triple_slash_references(source).into_iter().map(Requirement::TypesReference));
    }
    Ok(found)
}

/// Whether a path names a file that carries types rather than behaviour. The two
/// are parsed differently and reported differently, since a dependency reached
/// only from declarations breaks type checking while one reached from executable
/// code breaks the program.
pub fn is_declaration(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    name.ends_with(".d.ts") || name.ends_with(".d.mts") || name.ends_with(".d.cts")
}

/// What a file asks for. A `/// <reference types="x" />` is kept apart from an
/// import because TypeScript looks it up under `@types/` first, so the package
/// that satisfies it is usually not the name that was written.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Requirement {
    Module(String),
    TypesReference(String),
}

/// Declaration files are module syntax whatever their extension — a `.d.cts`
/// would otherwise be parsed as `CommonJS`. Executable code is ambiguous by
/// extension alone, so a `.js` that fails to parse one way is retried the other:
/// packages ship ESM under `.js` and `CommonJS` under `.mjs` often enough that
/// trusting the extension loses real files.
fn parse<'a>(
    allocator: &'a Allocator,
    source: &'a str,
    path: &Path,
) -> Result<oxc_ast::ast::Program<'a>, String> {
    if is_declaration(path) {
        return finish(Parser::new(allocator, source, SourceType::d_ts()).parse());
    }

    let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::mjs());
    let first = Parser::new(allocator, source, source_type).parse();
    if !first.panicked {
        return Ok(first.program);
    }
    finish(
        Parser::new(allocator, source, source_type.with_module(!source_type.is_module())).parse(),
    )
}

fn finish(parsed: oxc_parser::ParserReturn<'_>) -> Result<oxc_ast::ast::Program<'_>, String> {
    if parsed.panicked {
        return Err(parsed
            .diagnostics
            .first()
            .map_or_else(|| "unknown parse error".to_string(), ToString::to_string));
    }
    Ok(parsed.program)
}

#[derive(Default)]
struct Collector {
    found: BTreeSet<Requirement>,
}

impl Collector {
    fn record(&mut self, specifier: &str) {
        self.found.insert(Requirement::Module(specifier.to_string()));
    }
}

impl<'a> Visit<'a> for Collector {
    fn visit_import_declaration(&mut self, decl: &ImportDeclaration<'a>) {
        self.record(decl.source.value.as_str());
    }

    fn visit_export_from_declaration(&mut self, decl: &ExportFromDeclaration<'a>) {
        self.record(decl.source.value.as_str());
    }

    fn visit_export_all_declaration(&mut self, decl: &ExportAllDeclaration<'a>) {
        self.record(decl.source.value.as_str());
    }

    fn visit_import_expression(&mut self, expr: &ImportExpression<'a>) {
        if let Expression::StringLiteral(literal) = &expr.source {
            self.record(literal.value.as_str());
        }
        walk::walk_import_expression(self, expr);
    }

    fn visit_ts_import_type(&mut self, ty: &TSImportType<'a>) {
        self.record(ty.source.value.as_str());
        walk::walk_ts_import_type(self, ty);
    }

    fn visit_ts_external_module_reference(&mut self, reference: &TSExternalModuleReference<'a>) {
        self.record(reference.expression.value.as_str());
    }

    /// `require("pkg")` and `require.resolve("pkg")`. A call whose argument is
    /// not a literal names a package only the running program knows, so there is
    /// nothing to report.
    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if is_require(&call.callee) {
            if let Some(Argument::StringLiteral(literal)) = call.arguments.first() {
                self.record(literal.value.as_str());
            }
        }
        walk::walk_call_expression(self, call);
    }
}

fn is_require(callee: &Expression<'_>) -> bool {
    match callee {
        Expression::Identifier(name) => name.name == "require",
        Expression::StaticMemberExpression(member) => {
            member.property.name == "resolve" && is_require(&member.object)
        }
        _ => false,
    }
}

fn triple_slash_references(source: &str) -> Vec<String> {
    const MARKER: &str = "<reference types=";
    source
        .lines()
        .filter(|line| line.trim_start().starts_with("///"))
        .filter_map(|line| {
            let rest = line.split_once(MARKER)?.1.trim_start();
            let quote = rest.chars().next()?;
            rest[quote.len_utf8()..].split(quote).next().map(str::to_string)
        })
        .collect()
}

#[cfg(test)]
mod tests;
