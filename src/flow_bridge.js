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
function scope(parent, functionScope = false) { return { parent, functionScope, requireBound: false }; }
function functionParent(current) {
  while (current.parent && !current.functionScope) current = current.parent;
  return current;
}
function collect(node, current) {
  if (!node || typeof node !== 'object') return;
  if (Array.isArray(node)) { for (const item of node) collect(item, current); return; }
  if (node.type === 'FunctionDeclaration' && bindingContainsRequire(node.id)) current.requireBound = true;
  if (node.type === 'ClassDeclaration' && bindingContainsRequire(node.id)) current.requireBound = true;
  if (['FunctionDeclaration', 'FunctionExpression', 'ArrowFunctionExpression'].includes(node.type)) {
    current = scope(current, true);
    if (bindingContainsRequire(node.id) || node.params.some(bindingContainsRequire)) current.requireBound = true;
  } else if (node.type === 'ClassExpression') {
    current = scope(current);
    if (bindingContainsRequire(node.id)) current.requireBound = true;
  } else if (['BlockStatement', 'CatchClause', 'ForStatement', 'ForInStatement', 'ForOfStatement', 'SwitchStatement'].includes(node.type)) {
    current = scope(current);
    if (node.type === 'CatchClause' && bindingContainsRequire(node.param)) current.requireBound = true;
  }
  scopes.set(node, current);
  if (node.type === 'VariableDeclaration') {
    const bindingScope = node.kind === 'var' ? functionParent(current) : current;
    if (node.declarations.some(decl => bindingContainsRequire(decl.id))) bindingScope.requireBound = true;
    for (const decl of node.declarations) {
      if (decl.init && decl.init.type === 'CallExpression' &&
          (decl.init.callee.name === 'createRequire' || decl.init.callee.property?.name === 'createRequire')) {
        throw new Error('Flow createRequire binding needs loader analysis');
      }
    }
  }
  if (node.type === 'ImportDeclaration' && node.importKind !== 'type' && node.importKind !== 'typeof' &&
      node.specifiers.some(specifier => specifier.importKind !== 'type' && specifier.importKind !== 'typeof' && bindingContainsRequire(specifier.local))) current.requireBound = true;
  for (const [key, value] of Object.entries(node)) if (!skippedKeys.has(key)) collect(value, current);
}
function shadowed(current) {
  while (current) { if (current.requireBound) return true; current = current.parent; }
  return false;
}
function bindingContainsRequire(pattern) {
  if (!pattern || typeof pattern !== 'object') return false;
  if (pattern.type === 'Identifier') return pattern.name === 'require';
  if (pattern.type === 'AssignmentPattern') return bindingContainsRequire(pattern.left);
  if (pattern.type === 'RestElement') return bindingContainsRequire(pattern.argument);
  if (pattern.type === 'ObjectPattern') return pattern.properties.some(property => bindingContainsRequire(property.value || property.argument));
  if (pattern.type === 'ArrayPattern') return pattern.elements.some(bindingContainsRequire);
  return false;
}
function record(literal, origin) {
  if (literal && literal.type === 'Literal' && typeof literal.value === 'string') {
    found.push([literal.value, origin, literal.range[0]]);
  }
}
function isRequire(callee) {
  return callee && (callee.type === 'Identifier' && callee.name === 'require' ||
    callee.type === 'MemberExpression' && !callee.computed && callee.property.name === 'resolve' && isRequire(callee.object));
}
function visit(node) {
  if (!node || typeof node !== 'object') return;
  if (Array.isArray(node)) { for (const item of node) visit(item); return; }
  switch (node.type) {
    case 'ImportDeclaration': record(node.source, node.importKind === 'type' || node.importKind === 'typeof' ? 'types' : 'runtime'); break;
    case 'ExportNamedDeclaration': case 'ExportAllDeclaration': record(node.source, node.exportKind === 'type' ? 'types' : 'runtime'); break;
    case 'ImportExpression': record(node.source, 'runtime'); break;
    case 'CallExpression': if (isRequire(node.callee) && !shadowed(scopes.get(node))) record(node.arguments[0], 'runtime'); break;
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
