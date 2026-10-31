// The parser returns ESTree nodes and UTF-16 offsets. Only literal module
// references are reported; dynamic names are intentionally not guessed.
const fs = require('fs');
const parser = require(process.argv[1]);
const source = fs.readFileSync(0, 'utf8');
const ast = parser.parse(source, { types: true, enums: true });
if (ast.errors.length) throw new Error(ast.errors.map(error => error.message).join('; '));
const found = [];
// Collect lexical bindings before visiting calls so hoisted and later
// declarations shadow consistently. Block bindings remain local; var binds in
// the nearest function or program scope.
const scopes = new WeakMap();
const aliases = [];
const skippedKeys = new Set(['loc', 'range', 'comments', 'leadingComments', 'trailingComments', 'errors']);
function scope(parent, functionScope = false) { return { parent, functionScope, bound: new Map() }; }
function functionParent(current) {
  while (current.parent && !current.functionScope) current = current.parent;
  return current;
}
function collect(node, current) {
  if (!node || typeof node !== 'object') return;
  if (Array.isArray(node)) { for (const item of node) collect(item, current); return; }
  if (node.type === 'FunctionDeclaration') bind(current, node.id);
  if (node.type === 'ClassDeclaration') bind(current, node.id);
  if (['FunctionDeclaration', 'FunctionExpression', 'ArrowFunctionExpression'].includes(node.type)) {
    current = scope(current, true);
    bind(current, node.id); for (const param of node.params) bind(current, param);
  } else if (node.type === 'ClassExpression') {
    current = scope(current);
    bind(current, node.id);
  } else if (['BlockStatement', 'CatchClause', 'ForStatement', 'ForInStatement', 'ForOfStatement', 'SwitchStatement'].includes(node.type)) {
    current = scope(current);
    if (node.type === 'CatchClause') bind(current, node.param);
  }
  scopes.set(node, current);
  if (node.type === 'VariableDeclaration') {
    const bindingScope = node.kind === 'var' ? functionParent(current) : current;
    for (const decl of node.declarations) {
      bind(bindingScope, decl.id);
      if (node.kind === 'const' && decl.id.type === 'Identifier' && decl.init) aliases.push({ scope: bindingScope, name: decl.id.name, initializer: decl.init });
    }
    for (const decl of node.declarations) {
      if (decl.init && decl.init.type === 'CallExpression' &&
          (decl.init.callee.name === 'createRequire' || decl.init.callee.property?.name === 'createRequire')) {
        throw new Error('Flow createRequire binding needs loader analysis');
      }
    }
  }
  if (node.type === 'ImportDeclaration' && node.importKind !== 'type' && node.importKind !== 'typeof')
    for (const specifier of node.specifiers) if (specifier.importKind !== 'type' && specifier.importKind !== 'typeof') bind(current, specifier.local);
  for (const [key, value] of Object.entries(node)) if (!skippedKeys.has(key)) collect(value, current);
}
function shadowed(current, name) {
  while (current) { if (current.bound.has(name)) return true; current = current.parent; }
  return false;
}
function bind(current, pattern) {
  if (!pattern || typeof pattern !== 'object') return;
  switch (pattern.type) {
    case 'Identifier': current.bound.set(pattern.name, 'other'); break;
    case 'AssignmentPattern': bind(current, pattern.left); break;
    case 'RestElement': bind(current, pattern.argument); break;
    case 'ObjectPattern': for (const property of pattern.properties) bind(current, property.value || property.argument); break;
    case 'ArrayPattern': for (const element of pattern.elements) bind(current, element); break;
  }
}
function binding(current, name) {
  while (current) {
    if (current.bound.has(name)) return current.bound.get(name);
    current = current.parent;
  }
  return undefined;
}
function record(literal, origin, esm = false) {
  if (literal && literal.type === 'Literal' && typeof literal.value === 'string') {
    found.push([literal.value, origin, literal.range[0], esm]);
  }
}
function isLoader(callee, current) {
  if (!callee) return false;
  if (callee.type === 'Identifier') {
    const kind = binding(current, callee.name);
    return kind === 'loader' || (kind === undefined && callee.name === 'require');
  }
  if (callee.type !== 'MemberExpression') return false;
  const name = callee.computed ? callee.property.value : callee.property.name;
  return name === 'require' && callee.object.type === 'Identifier' && callee.object.name === 'module' && !shadowed(current, 'module');
}
function isRequire(callee, current) {
  if (isLoader(callee, current)) return true;
  if (!callee) return false;
  if (callee.type === 'Identifier') return binding(current, callee.name) === 'resolver';
  if (callee.type !== 'MemberExpression') return false;
  const name = callee.computed ? callee.property.value : callee.property.name;
  return name === 'resolve' && isLoader(callee.object, current);
}
function isImportMetaResolve(callee) {
  if (!callee || callee.type !== 'MemberExpression') return false;
  const name = callee.computed ? callee.property.value : callee.property.name;
  return name === 'resolve' && callee.object.type === 'MetaProperty' &&
    callee.object.meta.name === 'import' && callee.object.property.name === 'meta';
}
function hasLocalResolvePaths(node, current) {
  if (node.arguments.length < 2) return true;
  const options = node.arguments[1];
  if (options?.type !== 'ObjectExpression') return false;
  if (options.properties.length === 0) return true;
  if (options.properties.length !== 1) return false;
  const property = options.properties[0];
  const name = property?.key?.name ?? property?.key?.value;
  if (property?.type !== 'Property' || property.kind !== 'init' || property.computed || property.method || name !== 'paths') return false;
  const paths = property.value;
  return paths?.type === 'ArrayExpression' && paths.elements.length === 1 &&
    paths.elements[0]?.type === 'Identifier' && paths.elements[0].name === '__dirname' && !shadowed(current, '__dirname');
}
function visit(node) {
  if (!node || typeof node !== 'object') return;
  if (Array.isArray(node)) { for (const item of node) visit(item); return; }
  switch (node.type) {
    case 'ImportDeclaration': record(node.source, node.importKind === 'type' || node.importKind === 'typeof' ? 'types' : 'runtime', true); break;
    case 'ExportNamedDeclaration': case 'ExportAllDeclaration': record(node.source, node.exportKind === 'type' ? 'types' : 'runtime', true); break;
    case 'ImportExpression': record(node.source, 'runtime', true); break;
    case 'CallExpression': {
      const current = scopes.get(node);
      const loader = isLoader(node.callee, current);
      if (loader || (isRequire(node.callee, current) && hasLocalResolvePaths(node, current)) || isImportMetaResolve(node.callee)) record(node.arguments[0], 'runtime');
      break;
    }
  }
  for (const [key, value] of Object.entries(node)) {
    if (!skippedKeys.has(key)) visit(value);
  }
}
collect(ast, scope(null, true));
let changed;
do {
  changed = false;
  for (const alias of aliases) {
    if (alias.scope.bound.get(alias.name) !== 'other') continue;
    const kind = isLoader(alias.initializer, alias.scope) ? 'loader' : isRequire(alias.initializer, alias.scope) ? 'resolver' : undefined;
    if (kind) { alias.scope.bound.set(alias.name, kind); changed = true; }
  }
} while (changed);
visit(ast);
// Convert Flow's UTF-16 ranges in one pass, including on minified files with
// many references. Node strings index surrogate pairs as two code units.
found.sort((left, right) => left[2] - right[2]);
let units = 0;
let bytes = 0;
for (const item of found) {
  while (units < item[2]) {
    const character = String.fromCodePoint(source.codePointAt(units));
    units += character.length;
    bytes += Buffer.byteLength(character, 'utf8');
  }
  item[2] = bytes;
}
process.stdout.write(JSON.stringify(found));
