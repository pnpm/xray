// Find declaration errors whose installed @types package resolves from the
// importing package in an in-memory node_modules overlay. Never runs package JS.
const fs = require('node:fs');
const path = require('node:path');
const { createRequire } = require('node:module');
const { canonical, key } = require('./regenerate-paths.cjs');
const [projectArg, list] = process.argv.slice(2);
const real = p => { try { return canonical(p); } catch { return undefined; } };
const project = canonical(projectArg);
const roots = JSON.parse(fs.readFileSync(list, 'utf8')).map(x => ({ ...x, root: real(x.root) })).filter(x => x.root);
const inside = (p, root) => { const rel = path.relative(key(root), key(p)); return rel && rel !== '..' && !rel.startsWith('..' + path.sep) && !path.isAbsolute(rel); };
roots.sort((a, b) => b.root.length - a.root.length);
const providers = new Map();
for (const entry of roots) {
  const current = providers.get(entry.name) || [];
  current.push(entry);
  providers.set(entry.name, current);
}
let ts;
try { ts = createRequire(path.join(project, '__xray_types__.cjs'))('typescript'); } catch { console.log('[]'); process.exit(0); }
const config = ts.findConfigFile(project, ts.sys.fileExists, 'tsconfig.json');
if (!config || key(real(path.dirname(config))) !== key(real(project))) { console.log('[]'); process.exit(0); }
const contexts = [];
const visiting = new Set();
const visited = new Set();
function loadConfig(filename) {
  const canonical = real(filename);
  if (!canonical || !inside(canonical, real(project))) return false;
  const id = key(canonical);
  if (visiting.has(id)) return false;
  if (visited.has(id)) return true;
  visiting.add(id);
  const read = ts.readConfigFile(canonical, ts.sys.readFile);
  if (read.error) return false;
  const parsed = ts.parseJsonConfigFileContent(read.config, ts.sys, path.dirname(canonical), undefined, canonical);
  if (parsed.errors.length) return false;
  for (const ref of parsed.projectReferences || []) if (!loadConfig(ts.resolveProjectReferencePath(ref))) return false;
  visiting.delete(id);
  visited.add(id);
  contexts.push(parsed);
  return true;
}
if (!loadConfig(config)) { console.log('[]'); process.exit(0); }
const results = new Map();
for (const parsed of contexts) {
const options = { ...parsed.options, skipLibCheck: false, noEmit: true, incremental: false, composite: false };
const program = ts.createProgram(parsed.fileNames, options);
for (const diagnostic of ts.getPreEmitDiagnostics(program)) {
  if (![2307, 7016].includes(diagnostic.code) || !diagnostic.file || diagnostic.start === undefined || !/\.d\.(?:ts|mts|cts)$/.test(diagnostic.file.fileName)) continue;
  const file = real(diagnostic.file.fileName);
  if (!file) continue;
  const owner = roots.find(entry => inside(file, entry.root));
  if (!owner || owner.name.startsWith('@types/')) continue;
  let literal;
  function visit(node) {
    if (ts.isStringLiteral(node) && diagnostic.start >= node.getStart(diagnostic.file) && diagnostic.start < node.end &&
        (ts.isImportDeclaration(node.parent) || ts.isExportDeclaration(node.parent) || ts.isExternalModuleReference(node.parent) || ts.isLiteralTypeNode(node.parent))) literal = node;
    ts.forEachChild(node, visit);
  }
  visit(diagnostic.file);
  if (!literal) continue;
  const specifier = literal.text;
  const parts = specifier.split('/');
  const name = specifier.startsWith('@') ? parts.slice(0, 2).join('/') : parts[0];
  if (!name || name.startsWith('.') || name.startsWith('#') || name.startsWith('node:')) continue;
  let providerName;
  let kind;
  const runtime = ts.resolveModuleName(specifier, diagnostic.file.fileName, options, ts.sys).resolvedModule;
  const runtimeFile = runtime && real(runtime.resolvedFileName);
  const runtimeOwner = runtimeFile && roots.find(entry => inside(runtimeFile, entry.root));
  if (owner.declared.includes(name)) {
    providerName = '@types/' + (name.startsWith('@') ? name.slice(1).replace('/', '__') : name);
    kind = 'types';
    if (owner.declared.includes(providerName) || !runtimeFile || !runtimeOwner || runtimeOwner.name !== name || /\.d\.(?:ts|mts|cts)$/.test(runtimeFile)) continue;
  } else {
    if (diagnostic.code !== 2307 || runtimeFile) continue;
    providerName = name;
    kind = 'module';
  }
  const matches = providers.get(providerName) || [];
  const direct = real(path.join(project, 'node_modules', ...providerName.split('/')));
  const directMatches = direct && matches.filter(entry => key(entry.root) === key(direct));
  const provider = directMatches?.length === 1 ? directMatches[0] : matches.length === 1 ? matches[0] : undefined;
  if (!provider) continue;
  const virtual = path.join(owner.root, 'node_modules', ...providerName.split('/'));
  const syntheticDirs = new Set();
  for (let dir = path.dirname(virtual); inside(dir, owner.root); dir = path.dirname(dir)) syntheticDirs.add(key(dir));
  const map = p => {
    const normalized = path.normalize(p);
    if (key(normalized) === key(virtual)) return provider.root;
    if (inside(normalized, virtual)) return path.join(provider.root, path.relative(virtual, normalized));
    return p;
  };
  const host = {
    fileExists: p => ts.sys.fileExists(map(p)), readFile: p => ts.sys.readFile(map(p)),
    directoryExists: p => syntheticDirs.has(key(p)) || ts.sys.directoryExists(map(p)),
    getDirectories: p => ts.sys.getDirectories(map(p)),
    realpath: p => real(map(p)) || map(p),
    useCaseSensitiveFileNames: ts.sys.useCaseSensitiveFileNames,
  };
  const resolved = ts.resolveModuleName(specifier, diagnostic.file.fileName, options, host).resolvedModule;
  const target = resolved && real(map(resolved.resolvedFileName));
  if (!target || !inside(target, provider.root) || !/\.d\.(?:ts|mts|cts)$/.test(target)) continue;
  const item = { owner: owner.root, provider: provider.root, kind };
  results.set(key(owner.root) + '\0' + key(provider.root), item);
}
}
console.log(JSON.stringify([...results.values()]));
