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
try {
  for (const globalStore of [false, true]) {
    const label = globalStore ? 'global virtual store' : 'isolated virtual store'
    const root = join(temp, globalStore ? 'global' : 'isolated')
    const app = join(root, 'packages/app')
    await mkdir(app, { recursive: true })
    const settings = `packages:\n  - packages/*\nhoist: false\nminimumReleaseAge: 0\nenableGlobalVirtualStore: ${globalStore}\nstoreDir: ${join(temp, 'store')}\n`
    const workspace = join(root, 'pnpm-workspace.yaml')
    await writeFile(workspace, settings)
    await writeFile(join(app, 'package.json'), JSON.stringify({ name: 'xray-corpus-consumer', private: true, type: 'module', dependencies: { '@medplum/core': '5.2.13', '@types/node': '22.19.15', typescript: '5.9.3' } }))
    await writeFile(join(app, 'index.ts'), "import type { MedplumClient } from '@medplum/core';\nexport type Client = MedplumClient;\n")
    // This unrelated optional peer is not the issue under test.
    await writeFile(join(app, 'types.d.ts'), "declare module 'pdfmake/interfaces' { export type TDocumentDefinitions = any; export type TFontDictionary = any; export type CustomTableLayout = any; }\n")
    await writeFile(join(app, 'tsconfig.json'), JSON.stringify({ compilerOptions: { target: 'es2022', module: 'nodenext', moduleResolution: 'nodenext', noEmit: true }, include: ['index.ts', 'types.d.ts'] }))
    checked('pnpm', ['install', '--ignore-scripts'], root)
    const core = join(app, 'node_modules/@medplum/core')
    const reports = JSON.parse(checked(binary, ['--json', root], root))
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
    await writeFile(workspace, `${settings}${extension}`)
    checked('pnpm', ['install', '--ignore-scripts', '--no-frozen-lockfile'], root)
    checked(tsc, ['--noEmit'], app)
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
    const runtimeReports = JSON.parse(checked(binary, ['--json', runtimeRoot], runtimeRoot))
    if (!runtimeReports.some((report) => report.package === '@vue/consolidate@1.0.0' && report.findings.some((finding) => finding.dependency === 'pug'))) {
      throw new Error(`${label}: root project scan missed @vue/consolidate in the workspace`)
    }
    const runtimeActual = await realpath(consolidate)
    if (relative(runtimeRoot, runtimeActual).startsWith('..') !== globalStore) throw new Error(`${label}: unexpected package location ${runtimeActual}`)
    const beforeRender = run(process.execPath, ['render.cjs'], runtimeApp)
    if (beforeRender.status === 0 || !`${beforeRender.stdout}${beforeRender.stderr}`.includes("Cannot find module 'pug'")) throw new Error(`${label}: expected missing pug, got:\n${beforeRender.stdout}\n${beforeRender.stderr}`)
    const runtimeExtension = checked(binary, ['--package-extensions', '--package', consolidate], runtimeRoot)
    if (!runtimeExtension.includes("'pug'")) throw new Error(`${label}: extension did not include pug`)
    await writeFile(runtimeWorkspace, `${settings}${runtimeExtension}`)
    checked('pnpm', ['install', '--ignore-scripts', '--no-frozen-lockfile'], runtimeRoot)
    checked(process.execPath, ['render.cjs'], runtimeApp)
    console.log(`PASS ${label}: missing runtime pug fails before generated extension and renders afterward`)
  }
} finally {
  await rm(temp, { recursive: true, force: true })
}
