use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ExportAllDeclaration, ExportFromDeclaration, Expression, ImportDeclaration, ImportExpression,
    TSExternalModuleReference, TSImportType,
};
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_span::SourceType;
use std::collections::BTreeSet;

/// Every module specifier a declaration file asks for, including type-only
/// positions that never execute — `import type`, `import("pkg")` inside a type,
/// and `/// <reference types="pkg" />`.
///
/// Returns `None` when the file could not be parsed. Callers must report that
/// rather than treat it as a package with nothing to find.
pub fn specifiers(source: &str) -> Result<BTreeSet<Requirement>, String> {
    let allocator = Allocator::default();
    // Declaration files are module syntax whatever their extension, and a
    // `.d.cts` would otherwise be parsed as CommonJS.
    let source_type = SourceType::d_ts();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed.panicked {
        let reason = parsed
            .diagnostics
            .first()
            .map_or_else(|| "unknown parse error".to_string(), |error| error.to_string());
        return Err(reason);
    }

    let mut collector = Collector::default();
    collector.visit_program(&parsed.program);
    let mut found = collector.found;
    found.extend(triple_slash_references(source).into_iter().map(Requirement::TypesReference));
    Ok(found)
}

/// What a file asks for. A `/// <reference types="x" />` is kept apart from an
/// import because TypeScript looks it up under `@types/` first, so the package
/// that satisfies it is usually not the name that was written.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Requirement {
    Module(String),
    TypesReference(String),
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
mod tests {
    use super::*;

    fn found(source: &str) -> Vec<Requirement> {
        specifiers(source).unwrap().into_iter().collect()
    }

    fn modules(source: &str) -> Vec<String> {
        found(source)
            .into_iter()
            .filter_map(|requirement| match requirement {
                Requirement::Module(specifier) => Some(specifier),
                Requirement::TypesReference(_) => None,
            })
            .collect()
    }

    #[test]
    fn collects_every_module_position() {
        let source = r#"
            import type { A } from 'static-import';
            export { B } from 'export-from';
            export * from 'export-all';
            import x = require('require-import');
            declare const c: import('type-position').C;
            declare function load(): Promise<typeof import('dynamic-import')>;
        "#;
        assert_eq!(
            modules(source),
            [
                "dynamic-import",
                "export-all",
                "export-from",
                "require-import",
                "static-import",
                "type-position",
            ]
        );
    }

    #[test]
    fn collects_nested_type_position_imports() {
        let source = "declare const a: import('outer').A<import('inner').B>;";
        assert_eq!(modules(source), ["inner", "outer"]);
    }

    #[test]
    fn ignores_specifiers_inside_comments() {
        let source = "/** @example import { readJson } from '@medplum/definitions'; */\nexport {};";
        assert!(modules(source).is_empty());
    }

    #[test]
    fn collects_triple_slash_type_references() {
        let source = "/// <reference types=\"node\" />\nexport {};";
        assert_eq!(found(source), [Requirement::TypesReference("node".to_string())]);
    }

    #[test]
    fn reports_files_it_cannot_parse() {
        let source = "export =       function broken(): void;";
        assert!(specifiers(source).is_err());
    }
}
