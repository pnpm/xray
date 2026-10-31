// Propose declaration text in memory. This loads the project's TypeScript
// compiler but never imports or executes the package being analyzed.
const fs = require('node:fs');
const path = require('node:path');
const { createRequire } = require('node:module');
const { normalize, key } = require('./regenerate-paths.cjs');

const [projectArg, rootsJson] = process.argv.slice(2);
const project = normalize(fs.realpathSync(normalize(projectArg)));
const real = file => { try { return normalize(fs.realpathSync(normalize(file))); } catch { return undefined; } };
const samePath = (left, right) => left !== undefined && right !== undefined && key(left) === key(right);
const localRequire = createRequire(path.join(project, '__xray_regenerate__.cjs'));
const ts = localRequire('typescript');
function finish(files, messages = []) { console.log(JSON.stringify({ compiler: ts.version, files, messages })); }
function reject(reason, diagnostics = []) {
  const detail = diagnostics.slice(0, 8).map(d => `TS${d.code}: ${ts.flattenDiagnosticMessageText(d.messageText, ' ')}`);
  finish([], [reason, ...detail]);
  process.exit(0);
}
const roots = JSON.parse(fs.readFileSync(rootsJson, 'utf8')).map(p => normalize(fs.realpathSync(normalize(p)))).sort((a, b) => b.split(path.sep).length - a.split(path.sep).length);
const configPath = ts.findConfigFile(project, ts.sys.fileExists, 'tsconfig.json');
if (!configPath || !samePath(real(path.dirname(configPath)), project)) throw new Error('type regeneration requires a tsconfig.json in the target project');
const read = ts.readConfigFile(configPath, ts.sys.readFile);
if (read.error) throw new Error(ts.flattenDiagnosticMessageText(read.error.messageText, '\n'));
const parsed = ts.parseJsonConfigFileContent(read.config, ts.sys, project, undefined, configPath);
if (parsed.errors.length) throw new Error(ts.formatDiagnostics(parsed.errors, diagnosticHost()));
if (parsed.projectReferences?.length) reject('Declaration generation rejected: TypeScript project references are not supported.');
// Verification always checks declarations, regardless of the project's skipLibCheck.
const options = { ...parsed.options, skipLibCheck: false, noEmit: true, incremental: false, composite: false };
const original = ts.createProgram(parsed.fileNames, options);
const initial = ts.getPreEmitDiagnostics(original);
const syntactic = original.getSyntacticDiagnostics();
// TS1123 (empty variable declaration list) is reported by the semantic pass.
const syntax = initial.filter(d => d.file && /\.d\.(?:ts|mts|cts)$/.test(d.file.fileName) &&
  ([1039, 1046, 1123].includes(d.code) || syntactic.some(s => s.file === d.file && s.code === d.code && s.start === d.start)));
const contained = (file, root) => { const p = path.relative(root, file); return p !== '' && !p.startsWith(`..${path.sep}`) && p !== '..' && !path.isAbsolute(p); };
const candidates = new Map();
for (const diagnostic of syntax) {
  const declaration = real(diagnostic.file.fileName);
  const root = declaration && roots.find(root => contained(declaration, root));
  if (!root) continue;
  const runtime = declaration.replace(/\.d\.(mts|cts|ts)$/, (_, ext) => ext === 'mts' ? '.mjs' : ext === 'cts' ? '.cjs' : '.js');
  if (!fs.existsSync(runtime) || !samePath(real(runtime), runtime)) continue;
  const perFile = syntax.filter(item => samePath(real(item.file.fileName), declaration));
  // Treat declaration text as virtual TypeScript only for the specific
  // ambient-initializer failure, never for arbitrary malformed declarations.
  const virtual = perFile.some(item => item.code === 1039) && perFile.every(item => [1039, 1046].includes(item.code));
  const virtualFile = declaration.replace(/\.d\.(mts|cts|ts)$/, '.$1');
  if (virtual && fs.existsSync(virtualFile)) continue;
  candidates.set(declaration, { root, runtime, virtual, virtualFile });
}
if (!candidates.size || !initial.length) {
  finish([], syntax.length ? ['No eligible adjacent JavaScript source for the reported declaration errors.'] : []);
  process.exit(0);
}

const generated = new Map();
const emitOptions = { ...options, allowJs: true, checkJs: false, noEmit: false, declaration: true,
  emitDeclarationOnly: true, noEmitOnError: false, outDir: path.join(project, '__xray_output__'), rootDir: undefined };
const emitProgram = ts.createProgram([...candidates.values()].map(x => x.runtime), emitOptions);
const emitDiagnostics = ts.getPreEmitDiagnostics(emitProgram);
if (emitDiagnostics.length) reject('Declaration generation rejected: source compilation reported errors.', emitDiagnostics);
const jsChecker = emitProgram.getTypeChecker();
const output = new Map();
const emit = emitProgram.emit(undefined, (name, text, _bom, _onError, sourceFiles) => {
  if (!name.endsWith('.d.ts') && !name.endsWith('.d.mts') && !name.endsWith('.d.cts')) return;
  if (sourceFiles?.length === 1) output.set(key(real(sourceFiles[0].fileName)), text);
}, undefined, true);
if (emit.emitSkipped) reject('Declaration generation rejected: TypeScript skipped emit.', emit.diagnostics);
for (const [declaration, { runtime }] of candidates) {
  const replacement = output.get(key(runtime));
  if (!replacement) reject(`Declaration generation rejected: no declaration emitted for ${runtime}.`);
  generated.set(key(declaration), replacement);
}

// Re-emit the original declaration text as virtual TypeScript where it contains
// only unsupported ambient initializers. This retains declaration-only exports
// which cannot be reconstructed from JavaScript. The host never writes source.
const virtualSources = new Map([...candidates].filter(([, value]) => value.virtual)
  .map(([declaration, value]) => [key(value.virtualFile), fs.readFileSync(declaration, 'utf8')]));
if (virtualSources.size) {
  const virtualHost = ts.createCompilerHost(emitOptions);
  const fileExists = virtualHost.fileExists.bind(virtualHost);
  const readFile = virtualHost.readFile.bind(virtualHost);
  const getSourceFile = virtualHost.getSourceFile.bind(virtualHost);
  virtualHost.fileExists = file => virtualSources.has(key(file)) || fileExists(file);
  virtualHost.readFile = file => virtualSources.get(key(file)) ?? readFile(file);
  virtualHost.getSourceFile = (file, languageVersion, onError, shouldCreateNewSourceFile) => {
    const text = virtualSources.get(key(file));
    return text === undefined ? getSourceFile(file, languageVersion, onError, shouldCreateNewSourceFile) :
      ts.createSourceFile(file, text, languageVersion, true, ts.getScriptKindFromFileName(file));
  };
  const virtualProgram = ts.createProgram([...candidates.values()].filter(value => value.virtual).map(value => value.virtualFile), emitOptions, virtualHost);
  const diagnostics = ts.getPreEmitDiagnostics(virtualProgram);
  if (diagnostics.length) reject('Declaration generation rejected: virtual source compilation reported errors.', diagnostics);
  const virtualOutput = new Map();
  const virtualEmit = virtualProgram.emit(undefined, (name, text, _bom, _onError, sourceFiles) => {
    if (/\.d\.(?:ts|mts|cts)$/.test(name) && sourceFiles?.length === 1) virtualOutput.set(key(sourceFiles[0].fileName), text);
  }, undefined, true);
  if (virtualEmit.emitSkipped) reject('Declaration generation rejected: TypeScript skipped virtual source emit.', virtualEmit.diagnostics);
  for (const [declaration, value] of candidates) {
    if (!value.virtual) continue;
    const replacement = virtualOutput.get(key(value.virtualFile));
    if (!replacement) reject(`Declaration generation rejected: no declaration emitted for ${declaration}.`);
    generated.set(key(declaration), replacement);
  }
}

const host = ts.createCompilerHost(options);
const originalRead = host.readFile.bind(host);
const originalSource = host.getSourceFile.bind(host);
host.readFile = file => generated.get(key(real(file))) ?? originalRead(file);
host.getSourceFile = (file, languageVersion, onError, shouldCreateNewSourceFile) => {
  const text = generated.get(key(real(file)));
  return text === undefined ? originalSource(file, languageVersion, onError, shouldCreateNewSourceFile) :
    ts.createSourceFile(file, text, languageVersion, true, ts.getScriptKindFromFileName(file));
};
const fixed = ts.createProgram(parsed.fileNames, options, host);
const fixedDiagnostics = ts.getPreEmitDiagnostics(fixed);
if (fixedDiagnostics.length) reject('Declaration generation rejected: consumer verification reported errors.', fixedDiagnostics);
const fixedChecker = fixed.getTypeChecker();
const originalChecker = original.getTypeChecker();
const exportsOf = (checker, file) => {
  const symbol = checker.getSymbolAtLocation(file);
  return symbol && checker.getExportsOfModule(symbol).map(x => x.name).sort();
};
for (const [declaration, { runtime, virtual }] of candidates) {
  const source = emitProgram.getSourceFile(runtime);
  // Preserve the filename spelling used by the consumer's program, which may be symlinked.
  const decl = fixed.getSourceFiles().find(file => samePath(real(file.fileName), declaration));
  const oldFile = original.getSourceFiles().find(file => samePath(real(file.fileName), declaration));
  const oldSymbol = oldFile && originalChecker.getSymbolAtLocation(oldFile);
  const newSymbol = decl && fixedChecker.getSymbolAtLocation(decl);
  if (!oldSymbol || !newSymbol) reject(`Declaration generation rejected: cannot recover original exports for ${declaration}.`);
  const oldExports = originalChecker.getExportsOfModule(oldSymbol);
  const newExports = fixedChecker.getExportsOfModule(newSymbol);
  const actualFlags = (checker, symbol) => {
    try { return (symbol.flags & ts.SymbolFlags.Alias ? checker.getAliasedSymbol(symbol) : symbol).flags; }
    catch { return undefined; }
  };
  const actualSymbol = (checker, symbol) => symbol.flags & ts.SymbolFlags.Alias ? checker.getAliasedSymbol(symbol) : symbol;
  const shape = (checker, type, location, format) => {
    const properties = checker.getPropertiesOfType(type).map(property => ({
      name: property.name,
      optional: Boolean(property.flags & ts.SymbolFlags.Optional),
      readonly: Boolean(property.declarations?.some(declaration => ts.getCombinedModifierFlags(declaration) & ts.ModifierFlags.Readonly)),
      type: checker.typeToString(checker.getTypeOfSymbolAtLocation(property, location), undefined, format),
    })).sort((left, right) => left.name.localeCompare(right.name));
    const calls = checker.getSignaturesOfType(type, ts.SignatureKind.Call).map(signature => checker.signatureToString(signature, undefined, format)).sort();
    const constructors = checker.getSignaturesOfType(type, ts.SignatureKind.Construct).map(signature => checker.signatureToString(signature, undefined, format)).sort();
    return JSON.stringify({ properties, calls, constructors });
  };
  for (const oldExport of oldExports) {
    const oldFlags = actualFlags(originalChecker, oldExport);
    const next = newExports.find(value => value.name === oldExport.name);
    if (!next) reject(`Declaration generation rejected: existing export ${oldExport.name} would be removed from ${declaration}.`);
    const newFlags = actualFlags(fixedChecker, next);
    if (oldFlags === undefined || newFlags === undefined) reject(`Declaration generation rejected: cannot recover export ${oldExport.name} in ${declaration}.`);
    for (const mask of [ts.SymbolFlags.Type, ts.SymbolFlags.Value, ts.SymbolFlags.Namespace]) {
      if ((oldFlags & mask) && !(newFlags & mask)) reject(`Declaration generation rejected: existing export ${oldExport.name} would lose its type, value, or namespace in ${declaration}.`);
    }
    // Names and symbol kinds alone are insufficient: a generated `any` can
    // silently erase an existing typed API while the consumer still compiles.
    // Preserve the types of declarations whose own spans contain no syntax
    // diagnostic. A malformed declaration itself cannot provide such proof.
    const reliable = oldExport.declarations?.find(item => item.getSourceFile() === oldFile && !syntax.some(diagnostic =>
      diagnostic.file === oldFile && diagnostic.start !== undefined && diagnostic.start >= item.getFullStart() && diagnostic.start <= item.end));
    if (reliable) {
      const oldActual = actualSymbol(originalChecker, oldExport);
      const newActual = actualSymbol(fixedChecker, next);
      for (const mask of [ts.SymbolFlags.Type, ts.SymbolFlags.Value]) {
        if (!(oldFlags & mask)) continue;
        const oldType = mask === ts.SymbolFlags.Type ? originalChecker.getDeclaredTypeOfSymbol(oldActual) : originalChecker.getTypeOfSymbolAtLocation(oldActual, reliable);
        const newType = mask === ts.SymbolFlags.Type ? fixedChecker.getDeclaredTypeOfSymbol(newActual) : fixedChecker.getTypeOfSymbolAtLocation(newActual, decl);
        const format = ts.TypeFormatFlags.NoTruncation | ts.TypeFormatFlags.UseFullyQualifiedType;
        const oldText = originalChecker.typeToString(oldType, undefined, format);
        const newText = fixedChecker.typeToString(newType, undefined, format);
        // A previously unconstrained `any` may safely become more precise. An
        // empty object can gain members, but only if the replacement is still
        // an object (never `any`). Keep all other types identical.
        const refinement = oldText === 'any' || (oldText === '{}' && Boolean(newType.flags & ts.TypeFlags.Object));
        if (oldText !== newText && !refinement)
          reject(`Declaration generation rejected: existing export ${oldExport.name} would change type in ${declaration}.`);
        if (!refinement && oldType.flags & ts.TypeFlags.Object && newType.flags & ts.TypeFlags.Object &&
            shape(originalChecker, oldType, reliable, format) !== shape(fixedChecker, newType, decl, format))
          reject(`Declaration generation rejected: existing export ${oldExport.name} would change its members in ${declaration}.`);
      }
    }
  }
  const valueExports = (checker, file) => {
    const symbol = checker.getSymbolAtLocation(file);
    return symbol && checker.getExportsOfModule(symbol).filter(item => actualFlags(checker, item) & ts.SymbolFlags.Value).map(item => item.name).sort();
  };
  const jsExports = source && (virtual ? valueExports(jsChecker, source) : exportsOf(jsChecker, source));
  const declarationExports = decl && (virtual ? valueExports(fixedChecker, decl) : exportsOf(fixedChecker, decl));
  if (!jsExports || !declarationExports || JSON.stringify(jsExports) !== JSON.stringify(declarationExports)) {
    reject(`Declaration generation rejected: exports differ for ${runtime}.`);
  }
}
finish([...candidates].map(([file, value]) => ({
  root: value.root, file, original: fs.readFileSync(file, 'utf8'), replacement: generated.get(key(file)),
}))); 

function diagnosticHost() { return { getCanonicalFileName: x => x, getCurrentDirectory: () => project, getNewLine: () => '\n' }; }
