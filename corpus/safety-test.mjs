#!/usr/bin/env node
// Adversarial CLI checks use synthetic packages and never access the registry.
import { spawnSync } from 'node:child_process'
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const binary = resolve(process.argv[2] ?? fileURLToPath(new URL('../target/debug/xray', import.meta.url)))
const root = await mkdtemp(join(tmpdir(), 'xray-safety-'))
const pkg = join(root, 'node_modules/safety-fixture')
const workspace = join(root, 'pnpm-workspace.yaml')
function run(args) {
  const result = spawnSync(binary, args, { cwd: root, encoding: 'utf8', maxBuffer: 10 * 1024 * 1024, timeout: 30_000 })
  if (result.error) throw result.error
  return result
}
function assert(condition, message) {
  if (!condition) throw new Error(message)
}
try {
  await mkdir(pkg, { recursive: true })
  await writeFile(join(pkg, 'package.json'), JSON.stringify({ name: 'safety-fixture', version: '1.0.0', types: 'index.d.ts', main: 'index.js', devDependencies: { 'missing-types': '^1.0.0' } }))
  await writeFile(join(pkg, 'index.d.ts'), "import type { T } from 'missing-types'; export type { T };\n")
  await writeFile(join(pkg, 'index.js'), "module.exports = () => require('missing-runtime');\n")
  await mkdir(join(root, 'node_modules/missing-runtime'), { recursive: true })
  await writeFile(join(root, 'node_modules/missing-runtime/package.json'), JSON.stringify({ name: 'missing-runtime', version: '1.0.0' }))
  const conflict = "# preserve user settings\npackageExtensions:\n  'safety-fixture@1.0.0':\n    peerDependenciesMeta:\n      'missing-runtime':\n        optional: false\n"
  await writeFile(workspace, conflict)
  const dryRun = run(['--fix', '--dry-run', root])
  assert(dryRun.status === 0 && dryRun.stdout.includes('missing-types'), `dry run failed: ${dryRun.stderr}`)
  assert(await readFile(workspace, 'utf8') === conflict, 'dry run wrote to workspace')
  const conflicting = run(['--fix', root])
  assert(conflicting.status !== 0 && conflicting.stderr.includes('conflict'), `expected conflicting repair failure: ${conflicting.stderr}`)
  assert(await readFile(workspace, 'utf8') === conflict, 'conflicting repair partially wrote to workspace')
  console.log('PASS dry-run and conflicting extension preserve workspace bytes')

  const original = '# preserve on parse failure\npackages: []\n'
  await writeFile(workspace, original)
  await writeFile(join(pkg, 'index.js'), 'function malformed( {\n')
  const parseFailure = run(['--fix', root])
  assert(parseFailure.status === 2, `expected incomplete status 2, got ${parseFailure.status}: ${parseFailure.stdout}`)
  assert(await readFile(workspace, 'utf8') === original, 'parse failure partially wrote to workspace')
  console.log('PASS reached parse failure leaves workspace bytes unchanged')

  const healthy = join(root, 'node_modules/healthy-fixture')
  await mkdir(healthy, { recursive: true })
  await writeFile(join(healthy, 'package.json'), JSON.stringify({ name: 'healthy-fixture', version: '1.0.0', types: 'index.d.ts', devDependencies: { 'healthy-types': '^2.0.0' } }))
  await writeFile(join(healthy, 'index.d.ts'), "import type { T } from 'healthy-types'; export type { T };\n")
  const partial = run(['--fix', root])
  const partialConfig = await readFile(workspace, 'utf8')
  assert(partial.status === 2, `expected incomplete status after independent repair, got ${partial.status}: ${partial.stderr}`)
  assert(partialConfig.includes('healthy-fixture@1.0.0') && partialConfig.includes('healthy-types'), `healthy package not repaired: ${partialConfig}`)
  assert(!partialConfig.includes('safety-fixture@1.0.0') && !partialConfig.includes('missing-types'), `failed package received partial repair: ${partialConfig}`)
  console.log('PASS incomplete analysis excludes the failed package and repairs an independent package')
  await rm(healthy, { recursive: true })

  await rm(join(root, 'node_modules/missing-runtime'), { recursive: true })
  await writeFile(join(pkg, 'index.js'), "module.exports = () => require(['missing', 'runtime'].join('-'));\n")
  await writeFile(join(root, 'trace.cjs'), "console.log('child stdout marker'); console.error('child stderr marker'); try { require('safety-fixture')() } catch (e) { console.error('caught marker'); process.exitCode = 7 }\n")
  const trace = run([root, '--trace', '--', process.execPath, 'trace.cjs'])
  assert(trace.status === 7, `trace changed child status to ${trace.status}: ${trace.stderr}`)
  assert(trace.stdout.includes('child stdout marker') && trace.stderr.includes('child stderr marker') && trace.stderr.includes('caught marker'), `trace lost child output: ${trace.stdout}\n${trace.stderr}`)
  assert(trace.stderr.includes('safety-fixture@1.0.0') && trace.stderr.includes('missing-runtime'), `trace failed to attribute computed unknown module: ${trace.stderr}`)
  console.log('PASS trace preserves stdout, stderr, exit status and attributes unknown computed import')

  const planningRoot = join(root, 'planning')
  const planningModules = join(planningRoot, 'node_modules')
  await mkdir(join(planningModules, '@types/node'), { recursive: true })
  await writeFile(join(planningModules, '@types/node/package.json'), JSON.stringify({ name: '@types/node', version: '22.19.15' }))
  for (const [index, version] of ['^8.0.0', '^10.0.0', '^12.0.0', '^14.0.0', '^16.0.0'].entries()) {
    const dir = join(planningModules, `legacy-types-${index}`)
    await mkdir(dir, { recursive: true })
    await writeFile(join(dir, 'package.json'), JSON.stringify({ name: `legacy-types-${index}`, version: '1.0.0', types: 'index.d.ts', devDependencies: { '@types/node': version } }))
    await writeFile(join(dir, 'index.d.ts'), "import type {} from '@types/node';\n")
  }
  await writeFile(join(planningRoot, 'pnpm-workspace.yaml'), '# plan safety\npackages: []\n')
  const plan = run(['--fix', '--dry-run', planningRoot])
  assert(plan.status === 0, `provider plan failed: ${plan.stderr}`)
  for (let index = 0; index < 5; index++) assert(plan.stdout.includes(`legacy-types-${index}@1.0.0`), `provider plan omitted package ${index}: ${plan.stdout}`)
  assert(!/^    dependencies:/m.test(plan.stdout) && (plan.stdout.match(/peerDependencies:/g) ?? []).length === 5, `plan should reuse installed provider as peers instead of installing old type trees: ${plan.stdout}`)
  console.log('PASS repair plan reuses installed modern types provider for multiple historic dev ranges')

  const checkRoot = join(root, 'check')
  const checkPkg = join(checkRoot, 'node_modules/check-fixture')
  const checkWorkspace = join(checkRoot, 'pnpm-workspace.yaml')
  await mkdir(checkPkg, { recursive: true })
  await writeFile(join(checkPkg, 'package.json'), JSON.stringify({ name: 'check-fixture', version: '1.0.0', main: 'index.js' }))
  await writeFile(join(checkPkg, 'index.js'), "require('check-missing');\n")
  const emptyConfig = '# check must not write\npackages: []\n'
  await writeFile(checkWorkspace, emptyConfig)
  const findingsCheck = run(['--check', '--json', checkRoot])
  const findingsOutput = JSON.parse(findingsCheck.stdout)
  assert(findingsCheck.status === 1 && findingsOutput.complete && findingsOutput.packages.some((report) => report.package === 'check-fixture@1.0.0' && report.findings.some((finding) => finding.dependency === 'check-missing')), `expected check status 1 for finding: ${findingsCheck.stdout} ${findingsCheck.stderr}`)
  assert(await readFile(checkWorkspace, 'utf8') === emptyConfig, '--check changed workspace settings')
  const configured = `${emptyConfig}packageExtensions:\n  'check-fixture@1.0.0':\n    dependencies:\n      'check-missing': '*'\n`
  await writeFile(checkWorkspace, configured)
  const cleanCheck = run(['--check', '--json', checkRoot])
  assert(cleanCheck.status === 0 && JSON.parse(cleanCheck.stdout).complete, `expected clean configured check: ${cleanCheck.stdout} ${cleanCheck.stderr}`)
  await writeFile(join(checkPkg, 'broken.js'), 'function malformed( {\n')
  await writeFile(join(checkPkg, 'index.js'), "require('another-missing');\n")
  const incompleteCheck = run(['--check', '--json', checkRoot])
  const incompleteOutput = JSON.parse(incompleteCheck.stdout)
  assert(incompleteCheck.status === 2 && !incompleteOutput.complete && incompleteOutput.diagnostics.length > 0 && incompleteOutput.packages.some((report) => report.package === 'check-fixture@1.0.0'), `expected incomplete status 2 taking precedence over finding: ${incompleteCheck.stdout} ${incompleteCheck.stderr}`)
  assert(await readFile(checkWorkspace, 'utf8') === configured, 'incomplete --check changed workspace settings')
  console.log('PASS check status distinguishes findings, configured clean state, and incomplete scans')
} finally {
  await rm(root, { recursive: true, force: true })
}
