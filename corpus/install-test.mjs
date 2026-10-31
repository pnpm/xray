#!/usr/bin/env node
// Opt-in integration test: installs pinned registry packages with scripts disabled.
import { spawnSync } from 'node:child_process'
import { mkdtemp, mkdir, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const binary = resolve(process.argv[2] ?? fileURLToPath(new URL('../target/debug/xray', import.meta.url)))
const temp = await mkdtemp(join(tmpdir(), 'xray-install-'))
function run(command, args, cwd) {
  const result = spawnSync(command, args, { cwd, encoding: 'utf8', maxBuffer: 20 * 1024 * 1024, timeout: 180_000 })
  if (result.error) throw result.error
  return result
}
function checked(command, args, cwd) {
  const result = run(command, args, cwd)
  if (result.status !== 0) throw new Error(`${command} ${args.join(' ')} failed (${result.status}):\n${result.stdout}\n${result.stderr}`)
  return result.stdout
}
function scan(root, targetPackage) {
  const result = run(binary, ['--json', root], root)
  if (result.status !== 0 && result.status !== 2) throw new Error(`project scan failed (${result.status}): ${result.stderr}`)
  const output = JSON.parse(result.stdout)
  if (output.schemaVersion !== 1 || !Array.isArray(output.packages) || !Array.isArray(output.diagnostics) || output.complete !== (result.status === 0)) throw new Error(`inconsistent project scan envelope: ${result.stdout}`)
  if (output.diagnostics.some((diagnostic) => diagnostic.package === targetPackage)) throw new Error(`target package analysis incomplete: ${JSON.stringify(output.diagnostics)}`)
  if (output.diagnostics.length) console.log(`  project scan has unrelated diagnostics: ${JSON.stringify(output.diagnostics)}`)
  return output.packages
}
function assertRepaired(root, packageId, dependency) {
  const reports = scan(root, packageId)
  if (reports.some((report) => report.package === packageId && report.findings.some((finding) => finding.dependency === dependency))) {
    throw new Error(`${packageId} still reports repaired dependency ${dependency}`)
  }
}
async function applyFix(root, expected) {
  const workspace = join(root, 'pnpm-workspace.yaml')
  const before = await readFile(workspace, 'utf8')
  const dryRun = checked(binary, ['--fix', '--dry-run', root], root)
  if (!dryRun.includes(expected)) throw new Error(`dry-run did not select ${expected}: ${dryRun}`)
  if (await readFile(workspace, 'utf8') !== before) throw new Error('dry-run changed workspace settings')
  checked(binary, ['--fix', root], root)
  const after = await readFile(workspace, 'utf8')
  if (!after.includes(expected) || !after.includes('# corpus settings comment') || !after.includes('hoist: false')) throw new Error(`repair did not preserve settings or add ${expected}: ${after}`)
  checked(binary, ['--fix', root], root)
  if (await readFile(workspace, 'utf8') !== after) throw new Error('repeated repair changed workspace settings')
  return after
}
try {
  for (const globalStore of [false, true]) {
    const label = globalStore ? 'global virtual store' : 'isolated virtual store'
    const root = join(temp, globalStore ? 'global' : 'isolated')
    const app = join(root, 'packages/app')
    await mkdir(app, { recursive: true })
    const settings = `# corpus settings comment\npackages:\n  - packages/*\nhoist: false\nminimumReleaseAge: 0\nenableGlobalVirtualStore: ${globalStore}\nstoreDir: ${join(temp, 'store')}\n`
    const workspace = join(root, 'pnpm-workspace.yaml')
    await writeFile(workspace, settings)
    await writeFile(join(app, 'package.json'), JSON.stringify({ name: 'xray-corpus-consumer', private: true, type: 'module', dependencies: { '@medplum/core': '5.2.13', '@types/node': '22.19.15', typescript: '5.9.3' } }))
    await writeFile(join(app, 'index.ts'), "import type { MedplumClient } from '@medplum/core';\nexport type Client = MedplumClient;\n")
    // This unrelated optional peer is not the issue under test.
    await writeFile(join(app, 'types.d.ts'), "declare module 'pdfmake/interfaces' { export type TDocumentDefinitions = any; export type TFontDictionary = any; export type CustomTableLayout = any; }\n")
    await writeFile(join(app, 'tsconfig.json'), JSON.stringify({ compilerOptions: { target: 'es2022', module: 'nodenext', moduleResolution: 'nodenext', noEmit: true }, include: ['index.ts', 'types.d.ts'] }))
    checked('pnpm', ['install', '--ignore-scripts'], root)
    const core = join(app, 'node_modules/@medplum/core')
    const reports = scan(root, '@medplum/core@5.2.13')
    if (!reports.some((report) => report.package === '@medplum/core@5.2.13' && report.findings.some((finding) => finding.dependency === '@medplum/fhirtypes'))) {
      throw new Error(`${label}: root project scan missed @medplum/core in the workspace`)
    }
    const actual = await realpath(core)
    const outsideRoot = relative(root, actual).startsWith('..')
    if (outsideRoot !== globalStore) throw new Error(`${label}: unexpected package location ${actual}`)
    const tsc = join(app, 'node_modules/.bin/tsc')
    const before = run(tsc, ['--noEmit'], app)
    if (before.status === 0 || !`${before.stdout}${before.stderr}`.includes("Cannot find module '@medplum/fhirtypes'")) {
      throw new Error(`${label}: expected missing declaration failure, got:\n${before.stdout}\n${before.stderr}`)
    }
    const extension = checked(binary, ['--package-extensions', '--package', core], root)
    if (!extension.includes("'@medplum/fhirtypes'")) throw new Error(`${label}: extension did not include missing dependency`)
    await applyFix(root, '@medplum/fhirtypes')
    checked('pnpm', ['install', '--ignore-scripts', '--no-frozen-lockfile'], root)
    checked(tsc, ['--noEmit'], app)
    assertRepaired(root, '@medplum/core@5.2.13', '@medplum/fhirtypes')
    console.log(`PASS ${label}: missing declaration fails before generated extension and typechecks afterward`)

    const runtimeRoot = join(temp, globalStore ? 'runtime-global' : 'runtime-isolated')
    const runtimeApp = join(runtimeRoot, 'packages/app')
    await mkdir(runtimeApp, { recursive: true })
    const runtimeWorkspace = join(runtimeRoot, 'pnpm-workspace.yaml')
    await writeFile(runtimeWorkspace, settings)
    await writeFile(join(runtimeApp, 'package.json'), JSON.stringify({ name: 'xray-runtime-consumer', private: true, dependencies: { '@vue/consolidate': '1.0.0', pug: '3.0.3' } }))
    await writeFile(join(runtimeApp, 'render.cjs'), `require('@vue/consolidate').pug.render('p hello', {}, (err, html) => {
  if (err) { console.error(err); process.exitCode = 1; return }
  if (html.trim() !== '<p>hello</p>') { console.error(html); process.exitCode = 1 }
});
`)
    checked('pnpm', ['install', '--ignore-scripts'], runtimeRoot)
    const consolidate = join(runtimeApp, 'node_modules/@vue/consolidate')
    const runtimeReports = scan(runtimeRoot, '@vue/consolidate@1.0.0')
    if (!runtimeReports.some((report) => report.package === '@vue/consolidate@1.0.0' && report.findings.some((finding) => finding.dependency === 'pug'))) {
      throw new Error(`${label}: root project scan missed @vue/consolidate in the workspace`)
    }
    const runtimeActual = await realpath(consolidate)
    if (relative(runtimeRoot, runtimeActual).startsWith('..') !== globalStore) throw new Error(`${label}: unexpected package location ${runtimeActual}`)
    const beforeRender = run(process.execPath, ['render.cjs'], runtimeApp)
    if (beforeRender.status === 0 || !`${beforeRender.stdout}${beforeRender.stderr}`.includes("Cannot find module 'pug'")) throw new Error(`${label}: expected missing pug, got:\n${beforeRender.stdout}\n${beforeRender.stderr}`)
    const beforeTrace = run(binary, [runtimeRoot, '--trace', '--', process.execPath, 'packages/app/render.cjs'], runtimeRoot)
    if (beforeTrace.status !== beforeRender.status || !beforeTrace.stderr.includes("Cannot find module 'pug'") || !beforeTrace.stderr.includes('@vue/consolidate@1.0.0') || !beforeTrace.stderr.includes('pug')) {
      throw new Error(`${label}: trace did not report real failure and preserve command status (${beforeTrace.status}):\n${beforeTrace.stdout}\n${beforeTrace.stderr}`)
    }
    const runtimeExtension = checked(binary, ['--package-extensions', '--package', consolidate], runtimeRoot)
    if (!runtimeExtension.includes("'pug'")) throw new Error(`${label}: extension did not include pug`)
    const beforeTraceFix = await readFile(runtimeWorkspace, 'utf8')
    const traceArgs = [runtimeRoot, '--trace', '--fix', '--', process.execPath, 'packages/app/render.cjs']
    const traceDry = run(binary, [runtimeRoot, '--trace', '--fix', '--dry-run', '--', process.execPath, 'packages/app/render.cjs'], runtimeRoot)
    if (traceDry.status !== beforeRender.status || !traceDry.stdout.includes("'pug'") || await readFile(runtimeWorkspace, 'utf8') !== beforeTraceFix) throw new Error(`${label}: traced dry run did not select pug without writing: ${traceDry.stdout}\n${traceDry.stderr}`)
    const tracedFix = run(binary, traceArgs, runtimeRoot)
    if (tracedFix.status !== beforeRender.status) throw new Error(`${label}: trace fix changed child exit status: ${tracedFix.status}`)
    const afterTraceFix = await readFile(runtimeWorkspace, 'utf8')
    if (!afterTraceFix.includes("'pug'") || afterTraceFix.includes('then-pug') || !afterTraceFix.includes('# corpus settings comment')) throw new Error(`${label}: trace fix chose unexpected dependencies or lost settings: ${afterTraceFix}`)
    const repeatTraceFix = run(binary, traceArgs, runtimeRoot)
    if (repeatTraceFix.status !== beforeRender.status || await readFile(runtimeWorkspace, 'utf8') !== afterTraceFix) throw new Error(`${label}: trace fix was not idempotent`)
    checked('pnpm', ['install', '--ignore-scripts', '--no-frozen-lockfile'], runtimeRoot)
    checked(process.execPath, ['render.cjs'], runtimeApp)
    assertRepaired(runtimeRoot, '@vue/consolidate@1.0.0', 'pug')
    const afterTrace = run(binary, [runtimeRoot, '--trace', '--', process.execPath, 'packages/app/render.cjs'], runtimeRoot)
    if (afterTrace.status !== 0 || afterTrace.stderr.includes('Observed failed Node resolutions')) throw new Error(`${label}: traced command still fails after repair:\n${afterTrace.stdout}\n${afterTrace.stderr}`)
    console.log(`PASS ${label}: trace observes missing runtime pug; repair makes the traced command succeed`)

    const volarRoot = join(temp, globalStore ? 'volar-global' : 'volar-isolated')
    const volarApp = join(volarRoot, 'packages/app')
    await mkdir(volarApp, { recursive: true })
    const volarWorkspace = join(volarRoot, 'pnpm-workspace.yaml')
    await writeFile(volarWorkspace, settings)
    await writeFile(join(volarApp, 'package.json'), JSON.stringify({ name: 'xray-volar-consumer', private: true, dependencies: { '@volar/typescript': '2.4.28', typescript: '5.9.3' } }))
    await writeFile(join(volarApp, 'index.ts'), "import * as volar from '@volar/typescript';\nconsole.log(volar);\n")
    await writeFile(join(volarApp, 'tsconfig.json'), JSON.stringify({ compilerOptions: { target: 'es2022', module: 'nodenext', moduleResolution: 'nodenext', noEmit: true }, include: ['index.ts'] }))
    checked('pnpm', ['install', '--ignore-scripts'], volarRoot)
    const volar = join(volarApp, 'node_modules/@volar/typescript')
    const volarTsc = join(volarApp, 'node_modules/.bin/tsc')
    const beforeVolar = run(volarTsc, ['--noEmit'], volarApp)
    if (beforeVolar.status === 0 || !`${beforeVolar.stdout}${beforeVolar.stderr}`.includes("Cannot find module '@volar/language-service'")) throw new Error(`${label}: expected missing Volar type, got:\n${beforeVolar.stdout}\n${beforeVolar.stderr}`)
    const volarExtension = checked(binary, ['--package-extensions', '--package', volar], volarRoot)
    if (!volarExtension.includes("'@volar/language-service'")) throw new Error(`${label}: extension did not include Volar type dependency`)
    await applyFix(volarRoot, '@volar/language-service')
    checked('pnpm', ['install', '--ignore-scripts', '--no-frozen-lockfile'], volarRoot)
    checked(volarTsc, ['--noEmit'], volarApp)
    assertRepaired(volarRoot, '@volar/typescript@2.4.28', '@volar/language-service')
    console.log(`PASS ${label}: missing Volar declaration fails before generated extension and typechecks afterward`)

    const jssRoot = join(temp, globalStore ? 'jss-global' : 'jss-isolated')
    const jssApp = join(jssRoot, 'packages/app')
    await mkdir(jssApp, { recursive: true })
    const jssWorkspace = join(jssRoot, 'pnpm-workspace.yaml')
    const jssSettings = `${settings}ignoreCompatibilityDb: true\n`
    await writeFile(jssWorkspace, jssSettings)
    await writeFile(join(jssApp, 'package.json'), JSON.stringify({ name: 'xray-jss-consumer', private: true, dependencies: { 'jss-plugin-rule-value-function': '10.1.1', 'tiny-warning': '1.0.3' } }))
    await writeFile(join(jssApp, 'load.cjs'), "if (typeof require('jss-plugin-rule-value-function').default !== 'function') throw Error('invalid plugin');\n")
    checked('pnpm', ['install', '--ignore-scripts'], jssRoot)
    const jss = join(jssApp, 'node_modules/jss-plugin-rule-value-function')
    const beforeJss = run(process.execPath, ['load.cjs'], jssApp)
    if (beforeJss.status === 0 || !`${beforeJss.stdout}${beforeJss.stderr}`.includes("Cannot find module 'tiny-warning'")) throw new Error(`${label}: expected missing tiny-warning, got:\n${beforeJss.stdout}\n${beforeJss.stderr}`)
    const jssExtension = checked(binary, ['--package-extensions', '--package', jss], jssRoot)
    if (!jssExtension.includes("'tiny-warning'")) throw new Error(`${label}: extension did not include tiny-warning`)
    await applyFix(jssRoot, 'tiny-warning')
    checked('pnpm', ['install', '--ignore-scripts', '--no-frozen-lockfile'], jssRoot)
    checked(process.execPath, ['load.cjs'], jssApp)
    assertRepaired(jssRoot, 'jss-plugin-rule-value-function@10.1.1', 'tiny-warning')
    console.log(`PASS ${label}: undeclared runtime import fails before generated extension and loads afterward`)

    const rollupRoot = join(temp, globalStore ? 'rollup-global' : 'rollup-isolated')
    const rollupApp = join(rollupRoot, 'packages/app')
    await mkdir(rollupApp, { recursive: true })
    await writeFile(join(rollupRoot, 'pnpm-workspace.yaml'), settings)
    await writeFile(join(rollupApp, 'package.json'), JSON.stringify({ name: 'xray-rollup-consumer', private: true, dependencies: { '@rollup/plugin-dynamic-import-vars': '2.1.4', '@types/estree': '1.0.8', rollup: '4.64.9', typescript: '5.9.3' } }))
    await writeFile(join(rollupApp, 'index.ts'), "import dynamicImportVars from '@rollup/plugin-dynamic-import-vars';\nconsole.log(dynamicImportVars);\n")
    await writeFile(join(rollupApp, 'tsconfig.json'), JSON.stringify({ compilerOptions: { target: 'esnext', module: 'nodenext', moduleResolution: 'nodenext', types: [], noEmit: true }, include: ['index.ts'] }))
    checked('pnpm', ['install', '--ignore-scripts'], rollupRoot)
    const rollupTsc = join(rollupApp, 'node_modules/.bin/tsc')
    const beforeRollup = run(rollupTsc, ['--noEmit'], rollupApp)
    if (beforeRollup.status === 0 || !`${beforeRollup.stdout}${beforeRollup.stderr}`.includes("Cannot find module 'estree'")) throw new Error(`${label}: expected missing estree type, got:\n${beforeRollup.stdout}\n${beforeRollup.stderr}`)
    const fixedRollup = await applyFix(rollupRoot, '@types/estree')
    if (!fixedRollup.includes("'@rollup/plugin-dynamic-import-vars@2.1.4':")) throw new Error(`missing Rollup repair: ${fixedRollup}`)
    if (fixedRollup.includes("'estree':")) throw new Error(`${label}: repair guessed an estree runtime dependency`)
    checked('pnpm', ['install', '--ignore-scripts', '--no-frozen-lockfile'], rollupRoot)
    checked(rollupTsc, ['--noEmit'], rollupApp)
    assertRepaired(rollupRoot, '@rollup/plugin-dynamic-import-vars@2.1.4', 'estree')
    console.log(`PASS ${label}: missing @types provider fails before repair and typechecks afterward`)

    const cssRoot = join(temp, globalStore ? 'css-global' : 'css-isolated')
    const cssApp = join(cssRoot, 'packages/app')
    await mkdir(cssApp, { recursive: true })
    await writeFile(join(cssRoot, 'pnpm-workspace.yaml'), settings)
    await writeFile(join(cssApp, 'package.json'), JSON.stringify({ name: 'xray-css-consumer', private: true, type: 'module', dependencies: { '@material/button': '14.0.0', '@material/base': '14.0.0', '@material/animation': '14.0.0', sass: '1.94.0', vite: '7.2.2' } }))
    await writeFile(join(cssApp, 'index.html'), '<html><head></head><body><script type="module" src="/main.js"></script></body></html>\n')
    await writeFile(join(cssApp, 'main.js'), "import './style.scss';\n")
    await writeFile(join(cssApp, 'style.scss'), '@use "@material/button/mdc-button.import.scss";\n')
    checked('pnpm', ['install', '--ignore-scripts'], cssRoot)
    const vite = join(cssApp, 'node_modules/.bin/vite')
    const beforeCss = run(vite, ['build'], cssApp)
    if (beforeCss.status === 0 || !`${beforeCss.stdout}${beforeCss.stderr}`.includes("Can't find stylesheet to import")) throw new Error(`${label}: expected missing stylesheet, got:\n${beforeCss.stdout}\n${beforeCss.stderr}`)
    const cssConfig = await applyFix(cssRoot, '@material/base')
    if (!cssConfig.includes('@material/animation')) throw new Error(`${label}: repair omitted @material/animation: ${cssConfig}`)
    checked('pnpm', ['install', '--ignore-scripts', '--no-frozen-lockfile'], cssRoot)
    checked(vite, ['build'], cssApp)
    assertRepaired(cssRoot, '@material/button@14.0.0', '@material/base')
    assertRepaired(cssRoot, '@material/button@14.0.0', '@material/animation')
    console.log(`PASS ${label}: Sass build fails before generated extension and succeeds afterward`)

    const typedRoot = join(temp, globalStore ? 'typed-global' : 'typed-isolated')
    const typedApp = join(typedRoot, 'packages/app')
    await mkdir(typedApp, { recursive: true })
    await writeFile(join(typedRoot, 'pnpm-workspace.yaml'), settings)
    await writeFile(join(typedApp, 'package.json'), JSON.stringify({ name: 'xray-typed-consumer', private: true, dependencies: { 'typed-array-byte-length': '1.0.3', 'available-typed-arrays': '1.0.7' } }))
    await writeFile(join(typedApp, 'load.cjs'), "const length = require('typed-array-byte-length'); if (length(new Uint16Array(3)) !== 6) throw Error('wrong byte length');\n")
    checked('pnpm', ['install', '--ignore-scripts'], typedRoot)
    const beforeTyped = run(process.execPath, ['load.cjs'], typedApp)
    if (beforeTyped.status === 0 || !`${beforeTyped.stdout}${beforeTyped.stderr}`.includes("Cannot find module 'available-typed-arrays'")) throw new Error(`${label}: expected missing available-typed-arrays, got:\n${beforeTyped.stdout}\n${beforeTyped.stderr}`)
    await applyFix(typedRoot, 'available-typed-arrays')
    checked('pnpm', ['install', '--ignore-scripts', '--no-frozen-lockfile'], typedRoot)
    checked(process.execPath, ['load.cjs'], typedApp)
    assertRepaired(typedRoot, 'typed-array-byte-length@1.0.3', 'available-typed-arrays')
    console.log(`PASS ${label}: runtime devDependency fails before repair and loads afterward`)

    const firebaseRoot = join(temp, globalStore ? 'firebase-global' : 'firebase-isolated')
    const firebaseApp = join(firebaseRoot, 'packages/app')
    await mkdir(firebaseApp, { recursive: true })
    await writeFile(join(firebaseRoot, 'pnpm-workspace.yaml'), settings)
    await writeFile(join(firebaseApp, 'package.json'), JSON.stringify({ name: 'xray-firebase-consumer', private: true, dependencies: { '@firebase/analytics-types': '0.8.5', '@firebase/app-types': '0.9.6', typescript: '5.9.3' } }))
    await writeFile(join(firebaseApp, 'index.ts'), "import type { FirebaseAnalytics } from '@firebase/analytics-types'; export type Analytics = FirebaseAnalytics;\n")
    await writeFile(join(firebaseApp, 'tsconfig.json'), JSON.stringify({ compilerOptions: { target: 'es2022', module: 'nodenext', moduleResolution: 'nodenext', types: [], noEmit: true }, include: ['index.ts'] }))
    checked('pnpm', ['install', '--ignore-scripts'], firebaseRoot)
    const firebaseTsc = join(firebaseApp, 'node_modules/.bin/tsc')
    const beforeFirebase = run(firebaseTsc, ['--noEmit'], firebaseApp)
    if (beforeFirebase.status === 0 || !`${beforeFirebase.stdout}${beforeFirebase.stderr}`.includes("Cannot find module '@firebase/app-types'")) throw new Error(`${label}: expected missing Firebase type dependency, got:\n${beforeFirebase.stdout}\n${beforeFirebase.stderr}`)
    await applyFix(firebaseRoot, '@firebase/app-types')
    checked('pnpm', ['install', '--ignore-scripts', '--no-frozen-lockfile'], firebaseRoot)
    checked(firebaseTsc, ['--noEmit'], firebaseApp)
    assertRepaired(firebaseRoot, '@firebase/analytics-types@0.8.5', '@firebase/app-types')
    console.log(`PASS ${label}: undeclared type dependency fails before repair and typechecks afterward`)

    const browserRoot = join(temp, globalStore ? 'browser-global' : 'browser-isolated')
    const browserApp = join(browserRoot, 'packages/app')
    await mkdir(browserApp, { recursive: true })
    await writeFile(join(browserRoot, 'pnpm-workspace.yaml'), settings)
    await writeFile(join(browserApp, 'package.json'), JSON.stringify({ name: 'xray-browser-consumer', private: true, type: 'module', dependencies: { coinstring: '2.0.0', 'crypto-browserify': '2.1.8', esbuild: '0.25.12' } }))
    await writeFile(join(browserApp, 'index.js'), "import coinstring from 'coinstring'; console.log(coinstring);\n")
    checked('pnpm', ['install', '--ignore-scripts'], browserRoot)
    const esbuild = join(browserApp, 'node_modules/.bin/esbuild')
    const esbuildArgs = ['index.js', '--bundle', '--platform=browser', '--external:assert', '--external:buffer', '--external:util', '--outfile=out.js']
    const beforeBrowser = run(esbuild, esbuildArgs, browserApp)
    if (beforeBrowser.status === 0 || !`${beforeBrowser.stdout}${beforeBrowser.stderr}`.includes('Could not resolve \"crypto\"')) throw new Error(`${label}: expected missing browser mapping dependency: ${beforeBrowser.stdout}\n${beforeBrowser.stderr}`)
    await applyFix(browserRoot, 'crypto-browserify')
    checked('pnpm', ['install', '--ignore-scripts', '--no-frozen-lockfile'], browserRoot)
    checked(esbuild, esbuildArgs, browserApp)
    assertRepaired(browserRoot, 'coinstring@2.0.0', 'crypto-browserify')
    console.log(`PASS ${label}: browser bundling fails before repair and succeeds afterward with host builtins external`)

    for (const [nodeTypes, compatible] of [['22.14.1', true], ['24.19.2', false]]) {
      const regenRoot = join(temp, `unenv-${globalStore ? 'global' : 'isolated'}-${nodeTypes}`)
      await mkdir(regenRoot, { recursive: true })
      const regenWorkspace = join(regenRoot, 'pnpm-workspace.yaml')
      const regenSettings = `${settings}packageExtensions:\n  'undici-types@*':\n    peerDependencies:\n      '@types/node': '*'\n    peerDependenciesMeta:\n      '@types/node':\n        optional: true\n`
      await writeFile(regenWorkspace, regenSettings)
      await writeFile(join(regenRoot, 'package.json'), JSON.stringify({ name: 'xray-unenv-consumer', private: true, type: 'module', dependencies: { unenv: '2.0.0-rc.24', typescript: '5.9.3', '@types/node': nodeTypes } }))
      await writeFile(join(regenRoot, 'tsconfig.json'), JSON.stringify({ compilerOptions: { target: 'esnext', module: 'nodenext', moduleResolution: 'nodenext', noEmit: true }, include: ['index.ts'] }))
      await writeFile(join(regenRoot, 'index.ts'), "import process, { cwd, env } from 'unenv/node/process';\nimport constants, { EACCES } from 'unenv/node/constants';\nconst n: number = EACCES; const d: string = cwd(); const e: NodeJS.ProcessEnv = env;\nconsole.log(process, constants, n, d, e);\n")
      checked('pnpm', ['install', '--ignore-scripts'], regenRoot)
      const regenTsc = join(regenRoot, 'node_modules/.bin/tsc')
      const beforeRegen = run(regenTsc, ['--noEmit'], regenRoot)
      if (beforeRegen.status === 0 || !`${beforeRegen.stdout}${beforeRegen.stderr}`.includes('error TS1123')) throw new Error(`${label}: expected malformed unenv declarations: ${beforeRegen.stdout}\n${beforeRegen.stderr}`)
      const beforeSettings = await readFile(regenWorkspace, 'utf8')
      const patchPath = join(regenRoot, 'patches/unenv@2.0.0-rc.24.patch')
      const preview = run(binary, ['--fix', '--regenerate-types', '--dry-run', regenRoot], regenRoot)
      if (preview.status !== 2 || preview.stdout.includes('unenv@2.0.0-rc.24') !== compatible || await readFile(regenWorkspace, 'utf8') !== beforeSettings || await readFile(patchPath, 'utf8').then(() => true, () => false)) throw new Error(`${label}: unexpected declaration repair preview for ${nodeTypes}: ${preview.stdout}\n${preview.stderr}`)
      const regenFix = run(binary, ['--fix', '--regenerate-types', regenRoot], regenRoot)
      if (regenFix.status !== 2) throw new Error(`${label}: declaration repair did not preserve incomplete status: ${regenFix.status} ${regenFix.stderr}`)
      if (!compatible) {
        if (await readFile(regenWorkspace, 'utf8') !== beforeSettings || await readFile(patchPath, 'utf8').then(() => true, () => false)) throw new Error(`${label}: incompatible declaration regeneration wrote changes`)
        console.log(`PASS ${label}: incompatible Node types refuse declaration patch without changes`)
        continue
      }
      const patch = await readFile(patchPath, 'utf8')
      if (!patch.includes('dist/runtime/node/process.d.mts') || !patch.includes('dist/runtime/node/constants.d.mts') || !(await readFile(regenWorkspace, 'utf8')).includes('patchedDependencies')) throw new Error(`${label}: expected both verified declarations and patch registration`)
      checked('pnpm', ['install', '--ignore-scripts', '--no-frozen-lockfile'], regenRoot)
      checked(regenTsc, ['--noEmit'], regenRoot)
      const afterScan = run(binary, ['--json', regenRoot], regenRoot)
      const afterOutput = JSON.parse(afterScan.stdout)
      if (afterScan.status !== 0 || !afterOutput.complete || afterOutput.diagnostics.length !== 0) throw new Error(`${label}: patched project scan remains incomplete: ${afterScan.stdout}\n${afterScan.stderr}`)
      console.log(`PASS ${label}: regenerated declarations repair unenv and typecheck after reinstall`)
      checked(binary, ['--fix', regenRoot], regenRoot)
      const guarded = join(regenRoot, 'node_modules/regeneration-guard')
      await mkdir(guarded, { recursive: true })
      await writeFile(join(guarded, 'package.json'), JSON.stringify({ name: 'regeneration-guard', version: '1.0.0', type: 'module', main: 'index.js', types: 'index.d.ts' }))
      await writeFile(join(guarded, 'index.js'), "export function used() { return 1 }\nexport function stable(value) { return value }\n")
      await writeFile(join(regenRoot, 'index.ts'), "import { used } from 'regeneration-guard'; console.log(used());\n")
      const guardedSettings = await readFile(regenWorkspace, 'utf8')
      const guardPath = join(regenRoot, 'patches/regeneration-guard@1.0.0.patch')
      for (const [name, declaration] of [
        ['function signature', 'export declare function used(): number;\nexport declare function stable(value: string): number;\nexport declare const;\n'],
        ['declaration-only interface', 'export declare function used(): number;\nexport interface PublicType { id: string }\nexport declare const;\n'],
      ]) {
        await writeFile(join(guarded, 'index.d.ts'), declaration)
        const refused = run(binary, ['--fix', '--regenerate-types', regenRoot], regenRoot)
        if (refused.status === 0 || !refused.stderr.includes('existing export') || await readFile(regenWorkspace, 'utf8') !== guardedSettings || await readFile(guardPath, 'utf8').then(() => true, () => false)) throw new Error(`${label}: unsafe ${name} regeneration was not rejected: ${refused.status} ${refused.stderr}`)
      }
      await writeFile(join(regenRoot, 'tsconfig.json'), JSON.stringify({ compilerOptions: { target: 'esnext', module: 'nodenext', moduleResolution: 'nodenext', noEmit: true }, references: [{ path: './other' }], include: ['index.ts'] }))
      const references = run(binary, ['--fix', '--regenerate-types', regenRoot], regenRoot)
      if (references.status === 0 || !references.stderr.includes('project references are not supported') || await readFile(regenWorkspace, 'utf8') !== guardedSettings || await readFile(guardPath, 'utf8').then(() => true, () => false)) throw new Error(`${label}: project references were not rejected safely: ${references.status} ${references.stderr}`)
      console.log(`PASS ${label}: regeneration rejects API erosion and project references without writes`)
    }
  }
} finally {
  await rm(temp, { recursive: true, force: true })
}
