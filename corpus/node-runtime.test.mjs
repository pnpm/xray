import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

const bridge = new URL('../src/flow_bridge.js', import.meta.url)
const parser = fileURLToPath(new URL('../vendor/flow-parser/flow_parser.js', import.meta.url))

test('the supported Node floor executes the actual bundled Flow bridge without running source', async () => {
  const script = await readFile(bridge, 'utf8')
  const result = spawnSync(process.execPath, ['-e', script, parser], {
    input: "// @flow\nimport type { T } from 'xray-types';\nconst load = require;\nload('xray-runtime');\nthrow new Error('source must not execute');\n",
    encoding: 'utf8', timeout: 30_000,
  })
  assert.ifError(result.error)
  assert.equal(result.status, 0, result.stderr)
  const found = JSON.parse(result.stdout)
  assert.ok(found.some(([specifier, origin]) => specifier === 'xray-types' && origin === 'types'))
  assert.ok(found.some(([specifier, origin]) => specifier === 'xray-runtime' && origin === 'runtime'))
})

test('trace preload preserves child status on supported and older Node versions', async () => {
  const root = await mkdtemp(join(tmpdir(), 'xray-node-runtime-'))
  try {
    const events = join(root, 'events.jsonl')
    await writeFile(events, '')
    const hook = new URL('../src/trace-hook.mjs', import.meta.url).href
    const source = "process.stdout.write('child marker', () => { process.exitCode = 7 })"
    const baseline = spawnSync(process.execPath, ['-e', source], { encoding: 'utf8' })
    const traced = spawnSync(process.execPath, ['--import', hook, '-e', source], { encoding: 'utf8', env: { ...process.env, XRAY_TRACE_FILE: events } })
    assert.ifError(traced.error)
    assert.equal(traced.status, baseline.status)
    assert.equal(traced.stdout, baseline.stdout)
    assert.equal(await readFile(events, 'utf8'), '')
    const supported = spawnSync(process.execPath, ['-e', "process.exit(typeof require('node:module').registerHooks === 'function' ? 0 : 1)"]).status === 0
    if (supported) assert.equal(traced.stderr, baseline.stderr)
    else assert.match(traced.stderr, /requires Node.js 22.15 or newer/)
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})
