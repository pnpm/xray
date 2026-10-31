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
const skippedKeys = new Set(['loc', 'range', 'comments', 'leadingComments', 'trailingComments', 'errors']);
function scope(parent, functionScope = false) { return { parent, functionScope, bound: new Set() }; }
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
    for (const decl of node.declarations) bind(bindingScope, decl.id);
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
function bindingContains(pattern, name) {
  if (!pattern || typeof pattern !== 'object') return false;
  if (pattern.type === 'Identifier') return pattern.name === name;
  if (pattern.type === 'AssignmentPattern') return bindingContains(pattern.left, name);
  if (pattern.type === 'RestElement') return bindingContains(pattern.argument, name);
  if (pattern.type === 'ObjectPattern') return pattern.properties.some(property => bindingContains(property.value || property.argument, name));
  if (pattern.type === 'ArrayPattern') return pattern.elements.some(element => bindingContains(element, name));
  return false;
}
function bind(current, pattern) {
  for (const name of ['require', 'module']) if (bindingContains(pattern, name)) current.bound.add(name);
}
function record(literal, origin, esm = false) {
  if (literal && literal.type === 'Literal' && typeof literal.value === 'string') {
    found.push([literal.value, origin, literal.range[0], esm]);
  }
}
function isRequire(callee, current) {
  if (!callee) return false;
  if (callee.type === 'Identifier') return callee.name === 'require' && !shadowed(current, 'require');
  if (callee.type !== 'MemberExpression') return false;
  const name = callee.computed ? callee.property.value : callee.property.name;
  if (name === 'resolve') return isRequire(callee.object, current);
  return name === 'require' && callee.object.type === 'Identifier' && callee.object.name === 'module' && !shadowed(current, 'module');
}
function visit(node) {
  if (!node || typeof node !== 'object') return;
  if (Array.isArray(node)) { for (const item of node) visit(item); return; }
  switch (node.type) {
    case 'ImportDeclaration': record(node.source, node.importKind === 'type' || node.importKind === 'typeof' ? 'types' : 'runtime', true); break;
    case 'ExportNamedDeclaration': case 'ExportAllDeclaration': record(node.source, node.exportKind === 'type' ? 'types' : 'runtime', true); break;
    case 'ImportExpression': record(node.source, 'runtime', true); break;
    case 'CallExpression': if (isRequire(node.callee, scopes.get(node))) record(node.arguments[0], 'runtime'); break;
  }
  for (const [key, value] of Object.entries(node)) {
    if (!skippedKeys.has(key)) visit(value);
  }
}
collect(ast, scope(null, true));
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
