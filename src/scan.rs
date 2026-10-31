use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, BindingPattern, CallExpression, ExportAllDeclaration, ExportFromDeclaration,
    Expression, ImportDeclaration, ImportDeclarationSpecifier, ImportExpression, Program,
    PropertyKey, TSExternalModuleReference, TSImportType, VariableDeclarator,
};
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_semantic::{Scoping, SemanticBuilder};
use oxc_span::SourceType;
use oxc_syntax::symbol::SymbolId;
use serde::Serialize;
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
pub fn specifiers(source: &str, path: &Path) -> Result<BTreeSet<(Requirement, Origin)>, String> {
    let allocator = Allocator::default();
    // Some published declaration generators emit `export = function name(...)`
    // signatures, which Oxc rejects. Parse the equivalent declaration on a
    // retry so imports elsewhere in the file are still scanned.
    let normalized = is_declaration(path).then(|| normalize_export_assignment(source));
    let parsed = match parse(&allocator, source, path) {
        Ok(parsed) => parsed,
        Err(error) => match normalized.as_deref().filter(|text| *text != source) {
            Some(text) => parse(&allocator, text, path).map_err(|_| error)?,
            None => return Err(error),
        },
    };

    // Everything in a declaration file is erased; elsewhere only the positions
    // TypeScript strips are.
    let default_origin = if is_declaration(path) { Origin::Types } else { Origin::Runtime };
    let semantic = SemanticBuilder::new_compiler().build(&parsed).semantic;
    let mut factories = RequireFactories {
        named: BTreeSet::new(),
        namespaces: BTreeSet::new(),
        scoping: semantic.scoping(),
    };
    factories.visit_program(&parsed);
    let mut require_bindings = RequireBindings {
        factories: factories.named,
        namespaces: factories.namespaces,
        scoping: semantic.scoping(),
        bindings: BTreeSet::new(),
    };
    require_bindings.visit_program(&parsed);
    let mut collector = Collector {
        default_origin,
        found: BTreeSet::new(),
        scoping: semantic.scoping(),
        require_bindings: require_bindings.bindings,
    };
    collector.visit_program(&parsed);
    let mut found = collector.found;
    if is_declaration(path) {
        found.extend(
            triple_slash_references(&parsed)
                .into_iter()
                .map(|name| (Requirement::TypesReference(name), Origin::Types)),
        );
    }
    Ok(found)
}

/// Where a reference was found. A dependency reached from executable code
/// breaks the program when it is missing; one reached only from erased
/// positions breaks type checking instead, which no runtime detector can
/// observe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    Runtime,
    Types,
    Both,
}

impl Origin {
    pub fn merged(self, other: Self) -> Self {
        if self == other {
            self
        } else {
            Self::Both
        }
    }
}

/// Extensions this scanner can read: what a package ships as behaviour, and
/// what it ships as types. A manifest can point at a `.json` or an asset, and
/// those are not source.
const SCANNABLE: &[&str] = &[".js", ".mjs", ".cjs", ".jsx", ".ts", ".mts", ".cts", ".tsx"];

/// Whether a file is worth handing to the parser at all.
pub fn is_scannable(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    SCANNABLE.iter().any(|extension| name.ends_with(extension))
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
    let alternate = source_type.with_module(!source_type.is_module());
    let second = Parser::new(allocator, source, alternate).parse();
    if !second.panicked {
        return Ok(second.program);
    }
    // JSX is often published with a `.js` extension for bundlers.
    let jsx = Parser::new(allocator, source, source_type.with_jsx(true)).parse();
    if !jsx.panicked {
        return Ok(jsx.program);
    }
    finish(Parser::new(allocator, source, alternate.with_jsx(true)).parse())
}

fn normalize_export_assignment(source: &str) -> String {
    source
        .split_inclusive('\n')
        .map(|line| {
            let leading = line.len() - line.trim_start().len();
            let rest = &line[leading..];
            if let Some(after_export) = rest.strip_prefix("export") {
                let after_export = after_export.trim_start();
                if let Some(after_equals) = after_export.strip_prefix('=') {
                    let after_equals = after_equals.trim_start();
                    if after_equals.starts_with("function ") {
                        return format!("{}declare {after_equals}", &line[..leading]);
                    }
                }
            }
            line.to_string()
        })
        .collect()
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

struct Collector<'a> {
    default_origin: Origin,
    found: BTreeSet<(Requirement, Origin)>,
    scoping: &'a Scoping,
    require_bindings: BTreeSet<SymbolId>,
}

impl Collector<'_> {
    fn record(&mut self, specifier: &str) {
        let origin = self.default_origin;
        self.found.insert((Requirement::Module(specifier.to_string()), origin));
    }

    /// A position TypeScript erases: it needs the package to type check and
    /// never reaches it at run time, whatever kind of file it sits in.
    fn record_type_only(&mut self, specifier: &str) {
        self.found.insert((Requirement::Module(specifier.to_string()), Origin::Types));
    }
}

impl<'a> Visit<'a> for Collector<'_> {
    fn visit_import_declaration(&mut self, decl: &ImportDeclaration<'a>) {
        if decl.import_kind.is_type() {
            self.record_type_only(decl.source.value.as_str());
        } else {
            self.record(decl.source.value.as_str());
        }
    }

    fn visit_export_from_declaration(&mut self, decl: &ExportFromDeclaration<'a>) {
        if decl.export_kind.is_type() {
            self.record_type_only(decl.source.value.as_str());
        } else {
            self.record(decl.source.value.as_str());
        }
    }

    fn visit_export_all_declaration(&mut self, decl: &ExportAllDeclaration<'a>) {
        if decl.export_kind.is_type() {
            self.record_type_only(decl.source.value.as_str());
        } else {
            self.record(decl.source.value.as_str());
        }
    }

    fn visit_import_expression(&mut self, expr: &ImportExpression<'a>) {
        if let Expression::StringLiteral(literal) = &expr.source {
            self.record(literal.value.as_str());
        }
        walk::walk_import_expression(self, expr);
    }

    fn visit_ts_import_type(&mut self, ty: &TSImportType<'a>) {
        self.record_type_only(ty.source.value.as_str());
        walk::walk_ts_import_type(self, ty);
    }

    fn visit_ts_external_module_reference(&mut self, reference: &TSExternalModuleReference<'a>) {
        self.record(reference.expression.value.as_str());
    }

    /// `require("pkg")` and `require.resolve("pkg")`. A call whose argument is
    /// not a literal names a package only the running program knows, so there is
    /// nothing to report.
    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if is_require(&call.callee, self.scoping, &self.require_bindings) {
            if let Some(Argument::StringLiteral(literal)) = call.arguments.first() {
                self.record(literal.value.as_str());
            }
        }
        walk::walk_call_expression(self, call);
    }
}

fn is_require(
    callee: &Expression<'_>,
    scoping: &Scoping,
    require_bindings: &BTreeSet<SymbolId>,
) -> bool {
    match callee {
        Expression::Identifier(name) => {
            match name.reference_id.get().and_then(|id| scoping.get_reference(id).symbol_id()) {
                Some(id) => require_bindings.contains(&id),
                None => name.name == "require",
            }
        }
        Expression::StaticMemberExpression(member) => {
            member.property.name == "resolve"
                && is_require(&member.object, scoping, require_bindings)
        }
        _ => false,
    }
}

struct RequireFactories<'a> {
    named: BTreeSet<SymbolId>,
    namespaces: BTreeSet<SymbolId>,
    scoping: &'a Scoping,
}

impl<'a> Visit<'a> for RequireFactories<'_> {
    fn visit_import_declaration(&mut self, decl: &ImportDeclaration<'a>) {
        if matches!(decl.source.value.as_str(), "module" | "node:module") {
            if let Some(specifiers) = &decl.specifiers {
                for specifier in specifiers {
                    match specifier {
                        ImportDeclarationSpecifier::ImportSpecifier(specifier)
                            if specifier.imported.name() == "createRequire" =>
                        {
                            if let Some(id) = specifier.local.symbol_id.get() {
                                self.named.insert(id);
                            }
                        }
                        ImportDeclarationSpecifier::ImportNamespaceSpecifier(specifier) => {
                            if let Some(id) = specifier.local.symbol_id.get() {
                                self.namespaces.insert(id);
                            }
                        }
                        ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) => {
                            if let Some(id) = specifier.local.symbol_id.get() {
                                self.namespaces.insert(id);
                            }
                        }
                        ImportDeclarationSpecifier::ImportSpecifier(_) => {}
                    }
                }
            }
        }
        walk::walk_import_declaration(self, decl);
    }

    fn visit_variable_declarator(&mut self, decl: &VariableDeclarator<'a>) {
        if let (BindingPattern::ObjectPattern(pattern), Some(Expression::CallExpression(call))) =
            (&decl.id, &decl.init)
        {
            if is_node_module_load(call, self.scoping) {
                for property in &pattern.properties {
                    if matches!(&property.key, PropertyKey::StaticIdentifier(name) if name.name == "createRequire")
                    {
                        if let BindingPattern::BindingIdentifier(binding) = &property.value {
                            if let Some(id) = binding.symbol_id.get() {
                                self.named.insert(id);
                            }
                        }
                    }
                }
            }
        }
        if let (
            BindingPattern::BindingIdentifier(binding),
            Some(Expression::CallExpression(call)),
        ) = (&decl.id, &decl.init)
        {
            if is_node_module_load(call, self.scoping) {
                if let Some(id) = binding.symbol_id.get() {
                    self.namespaces.insert(id);
                }
            }
        }
        walk::walk_variable_declarator(self, decl);
    }
}

struct RequireBindings<'a> {
    factories: BTreeSet<SymbolId>,
    namespaces: BTreeSet<SymbolId>,
    scoping: &'a Scoping,
    bindings: BTreeSet<SymbolId>,
}

impl<'a> Visit<'a> for RequireBindings<'_> {
    fn visit_variable_declarator(&mut self, decl: &VariableDeclarator<'a>) {
        if let (
            BindingPattern::BindingIdentifier(binding),
            Some(Expression::CallExpression(call)),
        ) = (&decl.id, &decl.init)
        {
            let is_factory = match &call.callee {
                Expression::Identifier(name) => name
                    .reference_id
                    .get()
                    .and_then(|id| self.scoping.get_reference(id).symbol_id())
                    .is_some_and(|id| self.factories.contains(&id)),
                Expression::StaticMemberExpression(member) => {
                    member.property.name == "createRequire"
                        && match &member.object {
                            Expression::Identifier(name) => name
                                .reference_id
                                .get()
                                .and_then(|id| self.scoping.get_reference(id).symbol_id())
                                .is_some_and(|id| self.namespaces.contains(&id)),
                            Expression::CallExpression(call) => {
                                is_node_module_load(call, self.scoping)
                            }
                            _ => false,
                        }
                }
                _ => false,
            };
            if is_factory {
                if let Some(id) = binding.symbol_id.get() {
                    self.bindings.insert(id);
                }
            }
        }
        walk::walk_variable_declarator(self, decl);
    }
}

fn is_node_module_load(call: &CallExpression<'_>, scoping: &Scoping) -> bool {
    matches!(&call.callee, Expression::Identifier(_))
        && is_require(&call.callee, scoping, &BTreeSet::new())
        && matches!(call.arguments.first(), Some(Argument::StringLiteral(name)) if matches!(name.value.as_str(), "module" | "node:module"))
}

fn triple_slash_references(program: &Program<'_>) -> Vec<String> {
    const MARKER: &str = "<reference types=";
    program
        .comments
        .iter()
        .filter_map(|comment| {
            program.source_text.get(comment.span.start as usize..comment.span.end as usize)
        })
        .filter(|line| line.starts_with("///"))
        .filter_map(|line| {
            let rest = line.split_once(MARKER)?.1.trim_start();
            let quote = rest.chars().next()?;
            rest[quote.len_utf8()..].split(quote).next().map(str::to_string)
        })
        .collect()
}

#[cfg(test)]
mod tests;
