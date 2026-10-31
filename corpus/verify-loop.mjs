#!/usr/bin/env node
// Opt-in end-to-end tests. Uses pinned public packages and disables lifecycle scripts.
import { spawn, spawnSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { cp, mkdtemp, mkdir, readFile, readdir, realpath, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const binary = resolve(process.argv[2] ?? fileURLToPath(new URL('../target/debug/xray', import.meta.url)))
const temp = await mkdtemp(join(tmpdir(), 'xray-verify-loop-'))
const testEnv = { ...process.env, TMPDIR: temp }
function run(command, args, cwd, env = testEnv) {
  const result = spawnSync(command, args, { cwd, env, encoding: 'utf8', timeout: 240_000, maxBuffer: 20 * 1024 * 1024 })
  if (result.error) throw result.error
  return result
}
function checked(command, args, cwd, env) {
  const result = run(command, args, cwd, env)
  if (result.status !== 0) throw new Error(`${command} ${args.join(' ')} failed (${result.status}):\n${result.stdout}\n${result.stderr}`)
  return result
}
function assert(condition, message) { if (!condition) throw new Error(message) }
async function optionalFile(path) { return readFile(path, 'utf8').catch((error) => { if (error.code === 'ENOENT') return null; throw error }) }
async function snapshot(root) {
  return {
    config: await optionalFile(join(root, 'pnpm-workspace.yaml')),
    lock: await optionalFile(join(root, 'pnpm-lock.yaml')),
    memberLock: await optionalFile(join(root, 'packages/app/pnpm-lock.yaml')),
    patches: await Promise.all((await readdir(join(root, 'patches')).catch((error) => { if (error.code === 'ENOENT') return []; throw error })).sort().map(async (name) => [name, await readFile(join(root, 'patches', name), 'utf8')])),
  }
}
async function assertSnapshot(root, before, why) {
  const after = await snapshot(root)
  assert(JSON.stringify(after) === JSON.stringify(before), `${why}: workspace, locks, or patch artifacts changed`)
}
async function fixture(label, globalStore, separateLock = false) {
  const root = join(temp, label)
  const app = join(root, 'packages/app')
  await mkdir(app, { recursive: true })
  await mkdir(join(root, 'patches'))
  await writeFile(join(root, 'patches/preexisting.patch'), '# preexisting patch sentinel\n')
  await writeFile(join(root, 'pnpm-workspace.yaml'), `# preserve this policy\npackages:\n  - packages/*\nhoist: false\nignoreScripts: true\nenableGlobalVirtualStore: ${globalStore}\n${separateLock ? `sharedWorkspaceLockfile: false\n${globalStore ? '' : `virtualStoreDir: ${join(root, 'node_modules/.pnpm')}\n`}` : ''}minimumReleaseAge: 0\nstoreDir: ${join(temp, 'store')}\n`)
  await writeFile(join(app, 'package.json'), JSON.stringify({ name: `xray-verify-${label}`, private: true, dependencies: { '@vue/consolidate': '1.0.0', pug: '3.0.3', 'then-pug': '2.0.0' } }))
  await writeFile(join(app, 'render.cjs'), "require('@vue/consolidate').pug.render('p hello', {}, (err, html) => { if (err) { console.error(err); process.exitCode = 1 } else if (html.trim() !== '<p>hello</p>') process.exitCode = 2 });\n")
  await writeFile(join(app, 'unrelated.cjs'), "const fail = (err) => { if (err) console.error(err); else console.error('intentional unrelated failure'); process.exitCode = 73 }; process.on('unhandledRejection', fail); try { require('@vue/consolidate').pug.render('p hello', {}, fail) } catch (err) { fail(err) }\n")
  checked('pnpm', ['install', '--ignore-scripts'], root)
  const actual = await realpath(join(app, 'node_modules/@vue/consolidate'))
  assert(relative(root, actual).startsWith('..') === globalStore, `${label}: wrong virtual-store layout ${actual}`)
  return { root, app }
}
async function declarationFixture(label, globalStore) {
  const root = join(temp, label)
  await mkdir(join(root, 'patches'), { recursive: true })
  await writeFile(join(root, 'patches/preexisting.patch'), '# preexisting patch sentinel\n')
  await writeFile(join(root, 'pnpm-workspace.yaml'), `# preserve this policy\nhoist: false\nignoreScripts: true\nenableGlobalVirtualStore: ${globalStore}\nminimumReleaseAge: 0\nstoreDir: ${join(temp, 'store')}\n`)
  await writeFile(join(root, 'package.json'), JSON.stringify({ name: `xray-declarations-${label}`, private: true, type: 'module', dependencies: { 'v8-compile-cache-lib': '3.0.1', typescript: '5.9.3', '@types/node': '22.14.1' } }))
  await writeFile(join(root, 'tsconfig.json'), JSON.stringify({ compilerOptions: { target: 'esnext', module: 'nodenext', moduleResolution: 'nodenext', noEmit: true }, include: ['index.ts'] }))
  await writeFile(join(root, 'index.ts'), "import { install } from 'v8-compile-cache-lib'; const result = install({ cacheDir: '/tmp/example' }); result?.uninstall();\n")
  await writeFile(join(root, 'unrelated.cjs'), "const { spawnSync } = require('node:child_process'); const r = spawnSync(process.execPath, ['node_modules/typescript/bin/tsc', '--noEmit'], { stdio: 'inherit' }); if (r.error) console.error(r.error); process.exitCode = 73;\n")
  checked('pnpm', ['install', '--ignore-scripts'], root)
  const actual = await realpath(join(root, 'node_modules/v8-compile-cache-lib'))
  assert(relative(root, actual).startsWith('..') === globalStore, `${label}: wrong virtual-store layout ${actual}`)
  return root
}
try {
  for (const globalStore of [false, true]) {
    const label = globalStore ? 'global' : 'isolated'
    const { root, app } = await fixture(label, globalStore)
    const program = ['packages/app/render.cjs']
    const initial = run(process.execPath, program, root)
    assert(initial.status !== 0 && initial.stderr.includes("Cannot find module 'pug'"), `${label}: expected missing pug before repair: ${initial.stderr}`)
    const before = await snapshot(root)
    for (const [flag, value] of [['--verify-max-attempts', '0'], ['--verify-max-candidates', '257']]) {
      const invalid = run(binary, ['--verify', flag, value, root, '--', process.execPath, ...program], root)
      assert(invalid.status !== 0, `${label}: accepted invalid ${flag} ${value}`)
      await assertSnapshot(root, before, `${label}: invalid limit`)
    }
    const noop = run(binary, ['--verify', root, '--', process.execPath, '-e', "process.stdout.write('baseline success')"], root)
    assert(noop.status === 0 && noop.stdout.includes('baseline success'), `${label}: successful baseline failed: ${noop.stdout}\n${noop.stderr}`)
    await assertSnapshot(root, before, `${label}: baseline success`)
    const verified = run(binary, ['--verify', root, '--', process.execPath, ...program], root)
    assert(verified.status === 0, `${label}: verified repair failed: ${verified.stdout}\n${verified.stderr}`)
    checked(process.execPath, program, root)
    const config = await readFile(join(root, 'pnpm-workspace.yaml'), 'utf8')
    assert(config.includes('@vue/consolidate@1.0.0') && config.includes('# preserve this policy') && config.includes('hoist: false'), `${label}: missing minimal repair or original policy: ${config}`)
    const candidates = ['pug', 'then-pug'].filter((name) => new RegExp(`['"]${name}['"]`).test(config))
    assert(candidates.length === 1, `${label}: expected one sufficient adapter, got ${candidates}: ${config}`)
    console.log(`PASS ${label}: baseline success leaves no writes; verified minimal adapter repairs real render`)

    const bad = await fixture(`${label}-rollback`, globalStore)
    const original = await snapshot(bad.root)
    const unrelatedBaseline = run(process.execPath, ['packages/app/unrelated.cjs'], bad.root)
    assert(unrelatedBaseline.status === 73 && unrelatedBaseline.stderr.includes("Cannot find module 'pug'"), `${label}: rollback fixture must exit 73 with missing pug: ${unrelatedBaseline.status} ${unrelatedBaseline.stderr}`)
    const failed = run(binary, ['--verify', bad.root, '--', process.execPath, 'packages/app/unrelated.cjs'], bad.root)
    assert(failed.status === 73, `${label}: unrelated command failure lost status: ${failed.status}\n${failed.stderr}`)
    await assertSnapshot(bad.root, original, `${label}: failed verification`)
    const restored = run(process.execPath, ['packages/app/render.cjs'], bad.root)
    assert(restored.status !== 0 && restored.stderr.includes("Cannot find module 'pug'"), `${label}: original dependency graph not restored`)
    console.log(`PASS ${label}: unrelated failure restores settings, locks, patch artifacts, and original graph`)

    const separate = await fixture(`${label}-separate-lock`, globalStore, true)
    const separateBefore = await snapshot(separate.root)
    assert(separateBefore.lock === null && separateBefore.memberLock !== null, `${label}: expected only a member lockfile`)
    const separateBaseline = run(process.execPath, ['packages/app/unrelated.cjs'], separate.root)
    assert(separateBaseline.status === 73 && separateBaseline.stderr.includes("Cannot find module 'pug'"), `${label}: separate-lock baseline wrong: ${separateBaseline.status}`)
    const separateFailure = run(binary, ['--verify', separate.root, '--', process.execPath, 'packages/app/unrelated.cjs'], separate.root)
    assert(separateFailure.status === 73, `${label}: separate-lock rollback lost original status: ${separateFailure.stdout}\n${separateFailure.stderr}`)
    await assertSnapshot(separate.root, separateBefore, `${label}: separate-lock rollback`)
    const separateGraph = run(process.execPath, ['packages/app/render.cjs'], separate.root)
    assert(separateGraph.status !== 0 && separateGraph.stderr.includes("Cannot find module 'pug'"), `${label}: separate-lock graph not restored`)
    const separateSuccess = run(binary, ['--verify', separate.root, '--', process.execPath, 'packages/app/render.cjs'], separate.root)
    assert(separateSuccess.status === 0, `${label}: separate-lock repair failed: ${separateSuccess.stdout}\n${separateSuccess.stderr}`)
    checked(process.execPath, ['packages/app/render.cjs'], separate.root)
    const separateAfter = await snapshot(separate.root)
    assert(separateAfter.lock === null && separateAfter.memberLock !== null && separateAfter.memberLock !== separateBefore.memberLock, `${label}: repair did not update only the member lockfile`)
    console.log(`PASS ${label}: separate importer lock rolls back exactly and updates after successful repair`)

    const noLock = await fixture(`${label}-no-lock`, globalStore)
    await rm(join(noLock.root, 'pnpm-lock.yaml'))
    const noLockBefore = await snapshot(noLock.root)
    const refused = run(binary, ['--verify', noLock.root, '--', process.execPath, 'packages/app/render.cjs'], noLock.root)
    assert(refused.status !== 0 && /lock/i.test(refused.stderr), `${label}: missing lockfile was not refused: ${refused.stdout}\n${refused.stderr}`)
    await assertSnapshot(noLock.root, noLockBefore, `${label}: missing-lock refusal`)
    const noLockGraph = run(process.execPath, ['packages/app/render.cjs'], noLock.root)
    assert(noLockGraph.status !== 0 && noLockGraph.stderr.includes("Cannot find module 'pug'"), `${label}: missing-lock refusal changed graph`)
    console.log(`PASS ${label}: missing lockfile refuses repair without workspace or graph changes`)

    const conflict = await fixture(`${label}-mutation`, globalStore)
    const conflictBefore = await snapshot(conflict.root)
    await writeFile(join(conflict.app, 'mutate.cjs'), "const fs = require('node:fs'); const fail = (err) => { if (err) console.error(err); else fs.appendFileSync('pnpm-workspace.yaml', '# user command mutation\\n'); process.exitCode = 73 }; process.on('unhandledRejection', fail); try { require('@vue/consolidate').pug.render('p hello', {}, fail) } catch (err) { fail(err) }\n")
    const mutationBaseline = run(process.execPath, ['packages/app/mutate.cjs'], conflict.root)
    assert(mutationBaseline.status === 73 && mutationBaseline.stderr.includes("Cannot find module 'pug'"), `${label}: mutation baseline must fail before modifying settings`)
    await assertSnapshot(conflict.root, conflictBefore, `${label}: mutation baseline`)
    const mutation = run(binary, ['--verify', conflict.root, '--', process.execPath, 'packages/app/mutate.cjs'], conflict.root)
    const currentConfig = await readFile(join(conflict.root, 'pnpm-workspace.yaml'), 'utf8')
    assert(mutation.status === 1 && currentConfig.includes('# user command mutation'), `${label}: conflicting command mutation was not preserved: ${mutation.stdout}\n${mutation.stderr}`)
    const recoveryPath = mutation.stderr.match(/(?:\/[a-zA-Z0-9._-]+)+\/xray-recovery-[a-zA-Z0-9._-]+/)?.[0]
    assert(recoveryPath, `${label}: recovery path was not reported: ${mutation.stderr}`)
    try {
      const recovery = JSON.parse(await readFile(join(recoveryPath, 'recovery.json'), 'utf8'))
      const savedConfig = recovery.files.find((entry) => entry.path === 'pnpm-workspace.yaml')
      assert(savedConfig?.before?.file && savedConfig.expected?.file && await readFile(join(recoveryPath, savedConfig.before.file), 'utf8') === conflictBefore.config, `${label}: recovery backup did not retain original config`)
      const conflictSnapshot = await snapshot(conflict.root)
      const wrongRoot = join(temp, `${label}-wrong-root`)
      await mkdir(wrongRoot)
      await writeFile(join(wrongRoot, 'pnpm-workspace.yaml'), '# wrong workspace\n')
      const wrongBefore = await snapshot(wrongRoot)
      const wrong = run(binary, ['--recover', recoveryPath, wrongRoot], wrongRoot)
      assert(wrong.status !== 0, `${label}: wrong workspace accepted recovery journal`)
      await assertSnapshot(wrongRoot, wrongBefore, `${label}: wrong recovery workspace`)
      await assertSnapshot(conflict.root, conflictSnapshot, `${label}: wrong recovery workspace affected original`)
      const corruptPath = join(temp, `${label}-corrupt-journal`)
      await cp(recoveryPath, corruptPath, { recursive: true })
      await writeFile(join(corruptPath, 'recovery.json'), '{invalid json')
      const corrupt = run(binary, ['--recover', corruptPath, conflict.root], conflict.root)
      assert(corrupt.status !== 0, `${label}: corrupt recovery journal accepted`)
      await assertSnapshot(conflict.root, conflictSnapshot, `${label}: corrupt recovery journal`)
      await writeFile(join(conflict.root, 'pnpm-workspace.yaml'), await readFile(join(recoveryPath, savedConfig.expected.file)))
      const recovered = run(binary, ['--recover', recoveryPath, conflict.root], conflict.root)
      assert(recovered.status === 0, `${label}: explicit recovery failed: ${recovered.stdout}\n${recovered.stderr}`)
      await assertSnapshot(conflict.root, conflictBefore, `${label}: recovered transaction`)
      assert(await optionalFile(join(recoveryPath, 'recovery.json')) === null, `${label}: recovered journal was not removed`)
      const recoveredGraph = run(process.execPath, ['packages/app/render.cjs'], conflict.root)
      assert(recoveredGraph.status !== 0 && recoveredGraph.stderr.includes("Cannot find module 'pug'"), `${label}: recovered graph was not restored`)
    } finally {
      await rm(recoveryPath, { recursive: true, force: true })
    }
    console.log(`PASS ${label}: conflict preserves user change; invalid journals refuse and explicit recovery restores original graph`)

    const installFailure = await fixture(`${label}-install-failure`, globalStore)
    const installBefore = await snapshot(installFailure.root)
    const fakeBin = join(temp, `fake-pnpm-${label}`)
    await mkdir(fakeBin, { recursive: true })
    await writeFile(join(fakeBin, 'pnpm'), '#!/bin/sh\nprintf "%s\\n" "$@" > "$XRAY_VERIFY_PNPM_LOG"\nexit 42\n', { mode: 0o755 })
    const installLog = join(temp, `pnpm-${label}.log`)
    const env = { ...testEnv, PATH: `${fakeBin}:${process.env.PATH}`, XRAY_VERIFY_PNPM_LOG: installLog }
    const failedInstall = run(binary, ['--verify', installFailure.root, '--', process.execPath, 'packages/app/render.cjs'], installFailure.root, env)
    const pnpmArgs = await readFile(installLog, 'utf8')
    assert(failedInstall.status === 1 && pnpmArgs.includes('install'), `${label}: simulated install failure did not propagate: ${failedInstall.stdout}\n${failedInstall.stderr}`)
    assert(!pnpmArgs.includes('--ignore-scripts') && !pnpmArgs.includes('--force') && !pnpmArgs.includes('--registry'), `${label}: verifier overrode install policy: ${pnpmArgs}`)
    await assertSnapshot(installFailure.root, installBefore, `${label}: failed install`)
    const afterFailedInstall = run(process.execPath, ['packages/app/render.cjs'], installFailure.root)
    assert(afterFailedInstall.status !== 0 && afterFailedInstall.stderr.includes("Cannot find module 'pug'"), `${label}: failed install did not preserve graph`)
    console.log(`PASS ${label}: install failure preserves policy, configuration, locks, and original graph`)

    const cancellation = await fixture(`${label}-cancel`, globalStore)
    const cancelBefore = await snapshot(cancellation.root)
    await writeFile(join(cancellation.app, 'block.cjs'), `const fs = require('node:fs');
const { spawn } = require('node:child_process');
const fail = (error) => { if (error) console.error(error); process.exitCode = 1 };
process.on('unhandledRejection', fail);
try { require('@vue/consolidate').pug.render('p hello', {}, (err) => {
  if (err) return fail(err);
  const worker = spawn(process.execPath, ['-e', "const fs=require('node:fs');setInterval(()=>fs.appendFileSync('verify-heartbeat','x'),25)"], { stdio: 'ignore' });
  fs.writeFileSync('verify-child.marker', String(worker.pid));
  setInterval(() => {}, 1000);
}) } catch (error) { fail(error) }
`)
    const initialCancel = run(process.execPath, ['packages/app/block.cjs'], cancellation.root)
    assert(initialCancel.status !== 0 && initialCancel.stderr.includes("Cannot find module 'pug'"), `${label}: cancellation fixture should fail before repair`)
    const child = spawn(binary, ['--verify', cancellation.root, '--', process.execPath, 'packages/app/block.cjs'], { cwd: cancellation.root, env: testEnv, stdio: ['ignore', 'pipe', 'pipe'] })
    let childOutput = ''
    let heartbeatAtRollback = null
    child.stdout.on('data', (chunk) => { childOutput += chunk.toString() })
    child.stderr.on('data', (chunk) => {
      childOutput += chunk.toString()
      if (heartbeatAtRollback === null && childOutput.includes('Reinstalling the original locked dependency graph.')) {
        try { heartbeatAtRollback = readFileSync(join(cancellation.root, 'verify-heartbeat'), 'utf8') } catch { /* Checked after exit. */ }
      }
    })
    let exited = false
    const completion = new Promise((resolveDone, reject) => { child.once('error', reject); child.once('close', (code, signal) => { exited = true; resolveDone({ code, signal }) }) })
    const marker = join(cancellation.root, 'verify-child.marker')
    const heartbeat = join(cancellation.root, 'verify-heartbeat')
    const deadline = Date.now() + 180_000
    while (await optionalFile(marker) === null && !exited && Date.now() < deadline) await new Promise((resolveWait) => setTimeout(resolveWait, 100))
    if (exited || await optionalFile(marker) === null) {
      if (!exited) child.kill('SIGKILL')
      await completion
      throw new Error(`${label}: verified command never reached provisional blocking phase: ${childOutput}`)
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 150))
    child.kill('SIGINT')
    const timeout = new Promise((_, reject) => setTimeout(() => reject(new Error(`${label}: cancellation timed out`)), 120_000).unref())
    const cancelled = await Promise.race([completion, timeout])
    assert(cancelled.code === 130, `${label}: cancellation exit was ${cancelled.code}/${cancelled.signal}: ${childOutput}`)
    await assertSnapshot(cancellation.root, cancelBefore, `${label}: cancelled verification`)
    const afterCancel = run(process.execPath, ['packages/app/render.cjs'], cancellation.root)
    assert(afterCancel.status !== 0 && afterCancel.stderr.includes("Cannot find module 'pug'"), `${label}: cancellation did not restore original graph`)
    const lastHeartbeat = await optionalFile(heartbeat)
    assert(heartbeatAtRollback !== null && heartbeatAtRollback === lastHeartbeat, `${label}: descendant was not stopped before rollback started`)
    await new Promise((resolveWait) => setTimeout(resolveWait, 300))
    assert(await optionalFile(heartbeat) === lastHeartbeat, `${label}: descendant process continued running after cancellation`)
    const recoveryDirectories = (await readdir(temp)).filter((entry) => entry.startsWith('xray-recovery-'))
    assert(recoveryDirectories.length === 0, `${label}: successful cancellation left recovery journals: ${recoveryDirectories}`)
    console.log(`PASS ${label}: SIGINT terminates the command group before restoring the original graph`)

    const docusRoot = join(temp, `${label}-docusaurus`)
    const docusApp = join(docusRoot, 'packages/app')
    await mkdir(docusApp, { recursive: true })
    await writeFile(join(docusRoot, 'pnpm-workspace.yaml'), `# preserve policy\npackages:\n  - packages/*\nhoist: false\nignoreScripts: true\nenableGlobalVirtualStore: ${globalStore}\nminimumReleaseAge: 0\nstoreDir: ${join(temp, 'store')}\n`)
    await writeFile(join(docusApp, 'package.json'), JSON.stringify({ name: `xray-docusaurus-${label}`, private: true, type: 'module', dependencies: { '@docusaurus/core': '3.10.2', '@docusaurus/types': '3.10.2', history: '5.3.0', typescript: '5.9.3', '@types/node': '22.14.1', '@types/mdast': '4.0.4', '@types/react': '18.3.24' } }))
    await writeFile(join(docusApp, 'tsconfig.json'), JSON.stringify({ compilerOptions: { typeRoots: ['./node_modules/@types'], target: 'es2022', module: 'nodenext', moduleResolution: 'nodenext', noEmit: true }, include: ['index.ts'] }))
    await writeFile(join(docusApp, 'index.ts'), "import type { default as Dispatcher } from '@docusaurus/core/lib/client/ClientLifecyclesDispatcher.js'; export type API = typeof Dispatcher;\n")
    checked('pnpm', ['install', '--ignore-scripts'], docusRoot)
    const docusReal = await realpath(join(docusApp, 'node_modules/@docusaurus/core'))
    assert(relative(docusRoot, docusReal).startsWith('..') === globalStore, `${label}: wrong Docusaurus virtual-store layout`)
    const docusTsc = join(docusApp, 'node_modules/typescript/bin/tsc')
    const beforeDocus = run(process.execPath, [docusTsc, '--noEmit'], docusApp)
    assert(beforeDocus.status !== 0 && beforeDocus.stdout.includes("Cannot find module 'history'") && beforeDocus.stdout.includes("Cannot find module '@docusaurus/types'"), `${label}: expected Docusaurus deep-path failures: ${beforeDocus.stdout}\n${beforeDocus.stderr}`)
    const verifiedDocus = run(binary, ['--verify', docusApp, '--', process.execPath, docusTsc, '--noEmit'], docusApp)
    assert(verifiedDocus.status === 0, `${label}: verified Docusaurus repair failed: ${verifiedDocus.stdout}\n${verifiedDocus.stderr}`)
    checked(process.execPath, [docusTsc, '--noEmit'], docusApp)
    const docusConfig = await readFile(join(docusRoot, 'pnpm-workspace.yaml'), 'utf8')
    assert(docusConfig.includes('@docusaurus/core@3.10.2') && docusConfig.includes('history') && docusConfig.includes('@docusaurus/types') && !docusConfig.includes('@generated/'), `${label}: incorrect Docusaurus repair: ${docusConfig}`)
    console.log(`PASS ${label}: verified compiler repairs public Docusaurus deep-path types without generated aliases`)

    const sandboxRoot = join(temp, `${label}-sandbox`)
    const sandboxApp = join(sandboxRoot, 'packages/app')
    await mkdir(sandboxApp, { recursive: true })
    await writeFile(join(sandboxRoot, 'pnpm-workspace.yaml'), `# preserve policy\npackages:\n  - packages/*\nhoist: false\nignoreScripts: true\nenableGlobalVirtualStore: ${globalStore}\nminimumReleaseAge: 0\nstoreDir: ${join(temp, 'store')}\n`)
    await writeFile(join(sandboxApp, 'package.json'), JSON.stringify({ name: `xray-sandbox-${label}`, private: true, type: 'module', dependencies: { '@vercel/sandbox': '1.8.0', '@types/ms': '2.1.0', typescript: '5.9.3', '@types/node': '22.14.1' } }))
    await writeFile(join(sandboxApp, 'tsconfig.json'), JSON.stringify({ compilerOptions: { strict: true, target: 'es2022', module: 'nodenext', moduleResolution: 'nodenext', noEmit: true }, include: ['index.ts'] }))
    await writeFile(join(sandboxApp, 'index.ts'), "import type { signInAndGetToken } from '@vercel/sandbox/dist/utils/dev-credentials.js'; export type API = typeof signInAndGetToken;\n")
    checked('pnpm', ['install', '--ignore-scripts'], sandboxRoot)
    const sandboxReal = await realpath(join(sandboxApp, 'node_modules/@vercel/sandbox'))
    assert(relative(sandboxRoot, sandboxReal).startsWith('..') === globalStore, `${label}: wrong sandbox virtual-store layout`)
    const sandboxTsc = join(sandboxApp, 'node_modules/typescript/bin/tsc')
    const beforeSandbox = run(process.execPath, [sandboxTsc, '--noEmit'], sandboxApp)
    assert(beforeSandbox.status !== 0 && beforeSandbox.stdout.includes("Could not find a declaration file for module 'ms'"), `${label}: expected sandbox TS7016: ${beforeSandbox.stdout}\n${beforeSandbox.stderr}`)
    const verifiedSandbox = run(binary, ['--verify', sandboxApp, '--', process.execPath, sandboxTsc, '--noEmit'], sandboxApp)
    assert(verifiedSandbox.status === 0, `${label}: verified sandbox repair failed: ${verifiedSandbox.stdout}\n${verifiedSandbox.stderr}`)
    checked(process.execPath, [sandboxTsc, '--noEmit'], sandboxApp)
    const sandboxConfig = await readFile(join(sandboxRoot, 'pnpm-workspace.yaml'), 'utf8')
    assert(sandboxConfig.includes('@vercel/sandbox@1.8.0') && sandboxConfig.includes('@types/ms') && sandboxConfig.includes('# preserve policy'), `${label}: missing sandbox type-provider repair: ${sandboxConfig}`)
    console.log(`PASS ${label}: verified compiler repairs missing types for an already declared runtime dependency`)

    const declarationRoot = await declarationFixture(`${label}-declaration`, globalStore)
    const tscArgs = ['node_modules/typescript/bin/tsc', '--noEmit']
    const beforeTypecheck = run(process.execPath, tscArgs, declarationRoot)
    assert(beforeTypecheck.status !== 0 && beforeTypecheck.stdout.includes('TS1036'), `${label}: expected malformed declaration before repair: ${beforeTypecheck.stdout}`)
    const declarationFix = run(binary, ['--verify', '--regenerate-types', declarationRoot, '--', process.execPath, ...tscArgs], declarationRoot)
    assert(declarationFix.status === 0, `${label}: verified declaration repair failed: ${declarationFix.stdout}\n${declarationFix.stderr}`)
    checked(process.execPath, tscArgs, declarationRoot)
    const declarationConfig = await readFile(join(declarationRoot, 'pnpm-workspace.yaml'), 'utf8')
    assert(declarationConfig.includes('v8-compile-cache-lib@3.0.1') && declarationConfig.includes('patchedDependencies'), `${label}: missing verified declaration patch: ${declarationConfig}`)
    console.log(`PASS ${label}: verifies declaration repair against actual TypeScript command`)

    const declarationFailure = await declarationFixture(`${label}-declaration-rollback`, globalStore)
    const declarationBefore = await snapshot(declarationFailure)
    const failedDeclaration = run(binary, ['--verify', '--regenerate-types', declarationFailure, '--', process.execPath, 'unrelated.cjs'], declarationFailure)
    assert(failedDeclaration.status === 73, `${label}: declaration verification did not preserve unrelated failure: ${failedDeclaration.status}\n${failedDeclaration.stderr}`)
    await assertSnapshot(declarationFailure, declarationBefore, `${label}: failed declaration verification`)
    const restoredTypecheck = run(process.execPath, tscArgs, declarationFailure)
    assert(restoredTypecheck.status !== 0 && restoredTypecheck.stdout.includes('TS1036'), `${label}: declaration graph not restored after failed verification`)
    console.log(`PASS ${label}: failed declaration verification removes new patch and restores original graph`)
  }
} finally {
  await rm(temp, { recursive: true, force: true })
}
