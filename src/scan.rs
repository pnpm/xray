use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, BindingPattern, CallExpression, ExportAllDeclaration,
    ExportFromDeclaration, Expression, ImportDeclaration, ImportDeclarationSpecifier,
    ImportExpression, ObjectPropertyKind, Program, PropertyKey, PropertyKind,
    TSImportEqualsDeclaration, TSImportType, TSModuleReference, VariableDeclaration,
    VariableDeclarationKind, VariableDeclarator,
};
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_semantic::{Scoping, SemanticBuilder};
use oxc_span::{GetSpan, SourceType};
use oxc_syntax::symbol::SymbolId;
use serde::Serialize;
use std::{
    collections::BTreeSet,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

/// Node decodes JavaScript as UTF-8 with replacement for invalid bytes.
/// Preserve that behavior for published scripts while keeping other formats strict.
pub fn read_source(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "js" | "cjs" | "mjs"))
    {
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    } else {
        String::from_utf8(bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }
}

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
    Ok(references(source, path)?
        .into_iter()
        .map(|(requirement, origin, _)| (requirement, origin))
        .collect())
}

/// References with the byte offset of the source literal (or directive).
pub fn references(
    source: &str,
    path: &Path,
) -> Result<BTreeSet<(Requirement, Origin, u32)>, String> {
    if is_stylesheet(path) {
        return crate::style::parse(path, source)?
            .into_iter()
            .map(|reference| {
                let offset = u32::try_from(reference.offset)
                    .map_err(|_| "stylesheet exceeds supported size".to_string())?;
                Ok((
                    Requirement::Style(reference.specifier, reference.kind),
                    Origin::Runtime,
                    offset,
                ))
            })
            .collect();
    }
    if !is_typescript(path) && has_flow_pragma(source) {
        return flow_references(source);
    }
    let allocator = Allocator::default();
    // Some published declaration generators emit `export = function name(...)`
    // signatures, which Oxc rejects. Parse the equivalent declaration on a
    // retry so imports elsewhere in the file are still scanned.
    let normalized = is_declaration(path).then(|| normalize_export_assignment(source));
    let parsed = match parse(&allocator, source, path) {
        Ok(parsed) => parsed,
        Err(error) => match normalized.as_deref().filter(|text| *text != source) {
            Some(text) => parse(&allocator, text, path).map_err(|_| error)?,
            None if !is_typescript(path) => {
                return flow_references(source)
                    .map_err(|flow_error| format!("{error}; Flow parser: {flow_error}"))
            }
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
        constant: false,
    };
    factories.visit_program(&parsed);
    let mut require_bindings = RequireBindings {
        factories: factories.named,
        namespaces: factories.namespaces,
        scoping: semantic.scoping(),
        bindings: BTreeSet::new(),
        resolvers: BTreeSet::new(),
        constant: false,
    };
    loop {
        let count = require_bindings.bindings.len() + require_bindings.resolvers.len();
        require_bindings.visit_program(&parsed);
        if count == require_bindings.bindings.len() + require_bindings.resolvers.len() {
            break;
        }
    }
    let mut collector = Collector {
        default_origin,
        found: BTreeSet::new(),
        scoping: semantic.scoping(),
        require_bindings: require_bindings.bindings,
        resolve_bindings: require_bindings.resolvers,
    };
    collector.visit_program(&parsed);
    let mut found = collector.found;
    if is_typescript(path) {
        found.extend(
            triple_slash_references(&parsed)
                .into_iter()
                .map(|(requirement, offset)| (requirement, Origin::Types, offset)),
        );
    }
    Ok(found)
}

fn has_flow_pragma(source: &str) -> bool {
    let mut in_block = false;
    for line in source.lines().take(10) {
        let line = line.trim_start();
        if let Some(comment) = line.strip_prefix("//") {
            if comment.trim_start().starts_with("@flow") {
                return true;
            }
        } else {
            let comment = if let Some(comment) = line.strip_prefix("/*") {
                in_block = true;
                comment
            } else if in_block {
                line
            } else {
                continue;
            };
            if comment.trim_start_matches([' ', '\t', '*']).starts_with("@flow") {
                return true;
            }
            if comment.contains("*/") {
                in_block = false;
            }
        }
    }
    false
}

fn flow_references(source: &str) -> Result<BTreeSet<(Requirement, Origin, u32)>, String> {
    let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
    std::fs::write(
        dir.path().join("flow_parser.js"),
        include_bytes!("../vendor/flow-parser/flow_parser.js"),
    )
    .map_err(|error| error.to_string())?;
    let mut child = Command::new("node")
        .arg("-e")
        .arg(include_str!("flow_bridge.js"))
        .arg(dir.path().join("flow_parser.js"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("starting Flow parser (Node.js required): {error}"))?;
    let mut stdin = child.stdin.take().ok_or("Flow parser stdin unavailable")?;
    let write_result = stdin.write_all(source.as_bytes());
    drop(stdin);
    let output =
        child.wait_with_output().map_err(|error| format!("waiting for Flow parser: {error}"))?;
    write_result.map_err(|error| format!("sending source to Flow parser: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Flow parser failed: {}",
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }
    let values: Vec<(String, String, u32, bool)> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("reading Flow parser output: {error}"))?;
    values
        .into_iter()
        .map(|(specifier, origin, offset, esm)| {
            let origin = match origin.as_str() {
                "types" => Origin::Types,
                "runtime" => Origin::Runtime,
                _ => return Err(format!("unknown Flow reference origin {origin}")),
            };
            let requirement =
                if esm && specifier.starts_with('.') && specifier.contains(['?', '#', '%']) {
                    Requirement::EsmRelative(specifier)
                } else {
                    Requirement::Module(specifier)
                };
            Ok((requirement, origin, offset))
        })
        .collect()
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
const SCANNABLE: &[&str] = &[
    ".js", ".mjs", ".cjs", ".jsx", ".ts", ".mts", ".cts", ".tsx", ".css", ".scss", ".sass", ".less",
];

/// Whether a file is worth handing to the parser at all.
pub fn is_scannable(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    SCANNABLE.iter().any(|extension| name.ends_with(extension))
}

pub fn is_stylesheet(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    [".css", ".scss", ".sass", ".less"].iter().any(|extension| name.ends_with(extension))
}

/// Whether a path names a file that carries types rather than behaviour. The two
/// are parsed differently and reported differently, since a dependency reached
/// only from declarations breaks type checking while one reached from executable
/// code breaks the program.
pub fn is_declaration(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    name.ends_with(".d.ts") || name.ends_with(".d.mts") || name.ends_with(".d.cts")
}

fn is_typescript(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    [".ts", ".mts", ".cts", ".tsx"].iter().any(|extension| name.ends_with(extension))
}

/// What a file asks for. A `/// <reference types="x" />` is kept apart from an
/// import because TypeScript looks it up under `@types/` first, so the package
/// that satisfies it is usually not the name that was written.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Requirement {
    Module(String),
    /// A relative ESM URL requiring query/fragment or percent decoding.
    EsmRelative(String),
    TypesReference(String),
    /// A triple-slash path is relative to its containing file, even without `./`.
    PathReference(String),
    /// A stylesheet import, whose bare path may resolve to a local Sass partial.
    Style(String, crate::style::StyleKind),
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
    if !first.panicked && first.diagnostics.is_empty() {
        return Ok(first.program);
    }
    let alternate = source_type.with_module(!source_type.is_module());
    let second = Parser::new(allocator, source, alternate).parse();
    if !second.panicked && second.diagnostics.is_empty() {
        return Ok(second.program);
    }
    // Node wraps CommonJS in a function, so a top-level return is valid there.
    if path.extension().is_some_and(|extension| extension == "js" || extension == "cjs") {
        let commonjs = Parser::new(allocator, source, source_type.with_commonjs(true)).parse();
        if !commonjs.panicked && commonjs.diagnostics.is_empty() {
            return Ok(commonjs.program);
        }
    }
    // JSX is often published with a `.js` extension for bundlers.
    let jsx = Parser::new(allocator, source, source_type.with_jsx(true)).parse();
    if !jsx.panicked && jsx.diagnostics.is_empty() {
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
                        let prefix_len = line.len() - leading - after_equals.len();
                        if prefix_len > "declare".len() {
                            return format!(
                                "{}declare{}{after_equals}",
                                &line[..leading],
                                " ".repeat(prefix_len - "declare".len()),
                            );
                        }
                    }
                }
            }
            line.to_string()
        })
        .collect()
}

fn finish(parsed: oxc_parser::ParserReturn<'_>) -> Result<oxc_ast::ast::Program<'_>, String> {
    if parsed.panicked || !parsed.diagnostics.is_empty() {
        return Err(parsed
            .diagnostics
            .first()
            .map_or_else(|| "unknown parse error".to_string(), ToString::to_string));
    }
    Ok(parsed.program)
}

struct Collector<'a> {
    default_origin: Origin,
    found: BTreeSet<(Requirement, Origin, u32)>,
    scoping: &'a Scoping,
    require_bindings: BTreeSet<SymbolId>,
    resolve_bindings: BTreeSet<SymbolId>,
}

impl Collector<'_> {
    fn record(&mut self, specifier: &str, offset: u32) {
        let origin = self.default_origin;
        self.found.insert((Requirement::Module(specifier.to_string()), origin, offset));
    }

    /// A position TypeScript erases: it needs the package to type check and
    /// never reaches it at run time, whatever kind of file it sits in.
    fn record_type_only(&mut self, specifier: &str, offset: u32) {
        self.found.insert((Requirement::Module(specifier.to_string()), Origin::Types, offset));
    }

    fn record_esm(&mut self, specifier: &str, offset: u32, origin: Origin) {
        let requirement = if specifier.starts_with('.') && specifier.contains(['?', '#', '%']) {
            Requirement::EsmRelative(specifier.to_string())
        } else {
            Requirement::Module(specifier.to_string())
        };
        self.found.insert((requirement, origin, offset));
    }
}

impl<'a> Visit<'a> for Collector<'_> {
    fn visit_import_declaration(&mut self, decl: &ImportDeclaration<'a>) {
        if decl.import_kind.is_type() {
            self.record_esm(decl.source.value.as_str(), decl.source.span.start, Origin::Types);
        } else {
            self.record_esm(
                decl.source.value.as_str(),
                decl.source.span.start,
                self.default_origin,
            );
        }
    }

    fn visit_export_from_declaration(&mut self, decl: &ExportFromDeclaration<'a>) {
        if decl.export_kind.is_type() {
            self.record_esm(decl.source.value.as_str(), decl.source.span.start, Origin::Types);
        } else {
            self.record_esm(
                decl.source.value.as_str(),
                decl.source.span.start,
                self.default_origin,
            );
        }
    }

    fn visit_export_all_declaration(&mut self, decl: &ExportAllDeclaration<'a>) {
        if decl.export_kind.is_type() {
            self.record_esm(decl.source.value.as_str(), decl.source.span.start, Origin::Types);
        } else {
            self.record_esm(
                decl.source.value.as_str(),
                decl.source.span.start,
                self.default_origin,
            );
        }
    }

    fn visit_import_expression(&mut self, expr: &ImportExpression<'a>) {
        if let Expression::StringLiteral(literal) = &expr.source {
            self.record_esm(literal.value.as_str(), literal.span.start, self.default_origin);
        }
        walk::walk_import_expression(self, expr);
    }

    fn visit_ts_import_type(&mut self, ty: &TSImportType<'a>) {
        self.record_type_only(ty.source.value.as_str(), ty.source.span.start);
        walk::walk_ts_import_type(self, ty);
    }

    /// `import type X = require("pkg")` is erased like `import type`.
    fn visit_ts_import_equals_declaration(&mut self, decl: &TSImportEqualsDeclaration<'a>) {
        if let TSModuleReference::ExternalModuleReference(reference) = &decl.module_reference {
            let specifier = reference.expression.value.as_str();
            let offset = reference.expression.span.start;
            if decl.import_kind.is_type() {
                self.record_type_only(specifier, offset);
            } else {
                self.record(specifier, offset);
            }
        }
    }

    /// `require("pkg")` and `require.resolve("pkg")`. A call whose argument is
    /// not a literal names a package only the running program knows, so there is
    /// nothing to report.
    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        let loader = is_loader(&call.callee, self.scoping, &self.require_bindings);
        let resolver = !loader
            && is_require(
                &call.callee,
                self.scoping,
                &self.require_bindings,
                &self.resolve_bindings,
            );
        if loader
            || (resolver && has_local_resolve_paths(call, self.scoping))
            || is_import_meta_resolve(&call.callee)
        {
            if let Some(Argument::StringLiteral(literal)) = call.arguments.first() {
                self.record(literal.value.as_str(), literal.span.start);
            }
        }
        walk::walk_call_expression(self, call);
    }
}

/// Custom search roots can belong to another package. Only a missing options
/// object, an empty object, or the caller's unshadowed __dirname is local.
fn has_local_resolve_paths(call: &CallExpression<'_>, scoping: &Scoping) -> bool {
    let Some(options) = call.arguments.get(1) else {
        return true;
    };
    let Argument::ObjectExpression(options) = options else {
        return false;
    };
    if options.properties.is_empty() {
        return true;
    }
    let [ObjectPropertyKind::ObjectProperty(property)] = options.properties.as_slice() else {
        return false;
    };
    let named_paths = match &property.key {
        PropertyKey::StaticIdentifier(name) => name.name == "paths",
        PropertyKey::StringLiteral(name) => name.value == "paths",
        _ => false,
    };
    if !named_paths || property.computed || property.method || property.kind != PropertyKind::Init {
        return false;
    }
    let Expression::ArrayExpression(paths) = &property.value else {
        return false;
    };
    let [ArrayExpressionElement::Identifier(name)] = paths.elements.as_slice() else {
        return false;
    };
    name.name == "__dirname"
        && name.reference_id.get().and_then(|id| scoping.get_reference(id).symbol_id()).is_none()
}

fn is_import_meta_resolve(callee: &Expression<'_>) -> bool {
    match callee {
        Expression::StaticMemberExpression(member) => {
            member.property.name == "resolve" && matches!(&member.object, Expression::ImportMeta(_))
        }
        Expression::ComputedMemberExpression(member) => {
            matches!(&member.expression, Expression::StringLiteral(name) if name.value == "resolve")
                && matches!(&member.object, Expression::ImportMeta(_))
        }
        _ => false,
    }
}

fn is_require(
    callee: &Expression<'_>,
    scoping: &Scoping,
    require_bindings: &BTreeSet<SymbolId>,
    resolve_bindings: &BTreeSet<SymbolId>,
) -> bool {
    if is_loader(callee, scoping, require_bindings) {
        return true;
    }
    match callee {
        Expression::Identifier(name) => name
            .reference_id
            .get()
            .and_then(|id| scoping.get_reference(id).symbol_id())
            .is_some_and(|id| resolve_bindings.contains(&id)),
        Expression::StaticMemberExpression(member) => {
            member.property.name == "resolve"
                && is_loader(&member.object, scoping, require_bindings)
        }
        Expression::ComputedMemberExpression(member) => {
            matches!(&member.expression, Expression::StringLiteral(name) if name.value == "resolve")
                && is_loader(&member.object, scoping, require_bindings)
        }
        _ => false,
    }
}

fn is_loader(callee: &Expression<'_>, scoping: &Scoping, bindings: &BTreeSet<SymbolId>) -> bool {
    match callee {
        Expression::Identifier(name) => {
            match name.reference_id.get().and_then(|id| scoping.get_reference(id).symbol_id()) {
                Some(id) => bindings.contains(&id),
                None => name.name == "require",
            }
        }
        Expression::StaticMemberExpression(member) => {
            member.property.name == "require" && is_global_module(&member.object, scoping)
        }
        Expression::ComputedMemberExpression(member) => {
            matches!(&member.expression, Expression::StringLiteral(name) if name.value == "require")
                && is_global_module(&member.object, scoping)
        }
        _ => false,
    }
}

fn is_global_module(expression: &Expression<'_>, scoping: &Scoping) -> bool {
    match expression {
        Expression::Identifier(name) => {
            name.name == "module"
                && name
                    .reference_id
                    .get()
                    .and_then(|id| scoping.get_reference(id).symbol_id())
                    .is_none()
        }
        _ => false,
    }
}

struct RequireFactories<'a> {
    named: BTreeSet<SymbolId>,
    namespaces: BTreeSet<SymbolId>,
    scoping: &'a Scoping,
    constant: bool,
}

impl<'a> Visit<'a> for RequireFactories<'_> {
    fn visit_variable_declaration(&mut self, decl: &VariableDeclaration<'a>) {
        let previous = self.constant;
        self.constant = decl.kind == VariableDeclarationKind::Const;
        walk::walk_variable_declaration(self, decl);
        self.constant = previous;
    }
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
        if !self.constant {
            walk::walk_variable_declarator(self, decl);
            return;
        }
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
    resolvers: BTreeSet<SymbolId>,
    constant: bool,
}

impl<'a> Visit<'a> for RequireBindings<'_> {
    fn visit_variable_declaration(&mut self, decl: &VariableDeclaration<'a>) {
        if decl.kind == VariableDeclarationKind::Const {
            for variable in &decl.declarations {
                if let (BindingPattern::BindingIdentifier(binding), Some(initializer)) =
                    (&variable.id, &variable.init)
                {
                    if let Some(id) = binding.symbol_id.get() {
                        if is_loader(initializer, self.scoping, &self.bindings) {
                            self.bindings.insert(id);
                        } else if is_require(
                            initializer,
                            self.scoping,
                            &self.bindings,
                            &self.resolvers,
                        ) {
                            self.resolvers.insert(id);
                        }
                    }
                }
            }
        }
        let previous = self.constant;
        self.constant = decl.kind == VariableDeclarationKind::Const;
        walk::walk_variable_declaration(self, decl);
        self.constant = previous;
    }
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
            if is_factory && self.constant && has_local_create_require_base(call, self.scoping) {
                if let Some(id) = binding.symbol_id.get() {
                    self.bindings.insert(id);
                }
            }
        }
        walk::walk_variable_declarator(self, decl);
    }
}

/// A loader based elsewhere resolves from that location, not its caller. Only
/// infer ownership when Node's factory is anchored to the current source file.
fn has_local_create_require_base(call: &CallExpression<'_>, scoping: &Scoping) -> bool {
    match call.arguments.first() {
        Some(Argument::Identifier(name)) => {
            name.name == "__filename"
                && name
                    .reference_id
                    .get()
                    .and_then(|id| scoping.get_reference(id).symbol_id())
                    .is_none()
        }
        Some(Argument::StaticMemberExpression(member)) => {
            member.property.name == "url" && matches!(&member.object, Expression::ImportMeta(_))
        }
        Some(Argument::ComputedMemberExpression(member)) => {
            matches!(&member.expression, Expression::StringLiteral(name) if name.value == "url")
                && matches!(&member.object, Expression::ImportMeta(_))
        }
        _ => false,
    }
}

fn is_node_module_load(call: &CallExpression<'_>, scoping: &Scoping) -> bool {
    matches!(&call.callee, Expression::Identifier(_))
        && is_require(&call.callee, scoping, &BTreeSet::new(), &BTreeSet::new())
        && matches!(call.arguments.first(), Some(Argument::StringLiteral(name)) if matches!(name.value.as_str(), "module" | "node:module"))
}

fn triple_slash_references(program: &Program<'_>) -> Vec<(Requirement, u32)> {
    let first_statement = program.body.first().map_or(u32::MAX, |statement| statement.span().start);
    program
        .comments
        .iter()
        .filter(|comment| comment.span.start < first_statement)
        .filter_map(|comment| {
            program
                .source_text
                .get(comment.span.start as usize..comment.span.end as usize)
                .map(|line| (line, comment.span.start))
        })
        .filter(|(line, _)| line.starts_with("///"))
        .filter_map(|(line, offset)| {
            let directive = line.strip_prefix("///")?.trim_start();
            let (rest, is_path) = if let Some(rest) = directive.strip_prefix("<reference types=") {
                (rest, false)
            } else {
                (directive.strip_prefix("<reference path=")?, true)
            };
            let rest = rest.trim_start();
            let quote = rest.chars().next()?;
            if quote != '\'' && quote != '"' {
                return None;
            }
            rest[quote.len_utf8()..].split_once(quote).map(|(name, _)| {
                let reference = if is_path {
                    Requirement::PathReference(name.to_string())
                } else {
                    Requirement::TypesReference(name.to_string())
                };
                (reference, offset)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
