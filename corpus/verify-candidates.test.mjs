import { strict as assert } from 'node:assert'
import { createHash } from 'node:crypto'
import { test } from 'node:test'
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { downloadVerifiedArchive, eligibleFinding, expectedMissing, expectedFromTarget, matchesSelectedFinding, outsideProject, readScan } from './verify-candidates.mjs'

test('restricts install candidates to portable publisher type ranges', () => {
  const valid = { dependency: '@scope/types', severity: 'devDependency', origin: 'types', declaredRange: '^2.1.0' }
  assert.equal(eligibleFinding(valid), true)
  for (const range of ['file:../private', 'workspace:*', 'https://example.com/a.tgz', '*', 'latest']) assert.equal(eligibleFinding({ ...valid, declaredRange: range }), false)
  assert.equal(eligibleFinding({ ...valid, origin: 'runtime' }), false)
  assert.equal(eligibleFinding({ ...valid, dependency: '../private' }), false)
})

test('requires a missing-module diagnostic for the selected package', () => {
  assert.equal(expectedMissing("index.d.ts(1,1): error TS2307: Cannot find module '@scope/types/subpath'", '@scope/types'), true)
  assert.equal(expectedMissing("error TS2688: Cannot find type definition file for 'node'", '@types/node'), true)
  assert.equal(expectedMissing("error TS2307: Cannot find module '@scope/example'", '@types/scope__example'), true)
  assert.equal(expectedMissing("error TS2307: Cannot find module 'different'", 'target'), false)
  assert.equal(expectedMissing("error TS2322: Type 'target' is not assignable", 'target'), false)
})


test('rejects inconsistent scanner status and completeness', () => {
  const complete = JSON.stringify({ schemaVersion: 1, complete: true, packages: [], diagnostics: [] })
  const incomplete = JSON.stringify({ schemaVersion: 1, complete: false, packages: [], diagnostics: [{}] })
  assert.ok(readScan({ status: 0, stdout: complete }))
  assert.ok(readScan({ status: 2, stdout: incomplete }))
  assert.equal(readScan({ status: 2, stdout: complete }), null)
  assert.equal(readScan({ status: 0, stdout: incomplete }), null)
  assert.equal(readScan({ status: 1, stdout: complete }), null)
})


test('requires the missing diagnostic to come from the target package', async () => {
  const root = await mkdtemp(join(tmpdir(), 'xray-verify-test-'))
  try {
    const target = join(root, 'node_modules/target')
    const other = join(root, 'node_modules/other')
    await mkdir(target, { recursive: true })
    await mkdir(other, { recursive: true })
    await mkdir(join(target, 'node_modules/nested'), { recursive: true })
    await writeFile(join(target, 'index.d.ts'), '')
    await writeFile(join(other, 'index.d.ts'), '')
    await writeFile(join(target, 'node_modules/nested/index.d.ts'), '')
    const line = (path) => `${path}(1,1): error TS2307: Cannot find module 'missing'`
    assert.equal(await expectedFromTarget(line('node_modules/target/index.d.ts'), 'missing', root, 'target'), true)
    assert.equal(await expectedFromTarget(line('node_modules/other/index.d.ts'), 'missing', root, 'target'), false)
    assert.equal(await expectedFromTarget(line('node_modules/target/node_modules/nested/index.d.ts'), 'missing', root, 'target'), false)
    assert.equal(await expectedFromTarget(line('/nonexistent/private.d.ts'), 'missing', root, 'target'), false)
  } finally { await rm(root, { recursive: true, force: true }) }
})


test('requires the installed finding to match untrusted candidate details', () => {
  const selected = { dependency: 'example', severity: 'devDependency', origin: 'types', declaredRange: '^1.2.3' }
  assert.equal(matchesSelectedFinding({ ...selected }, selected), true)
  assert.equal(matchesSelectedFinding({ ...selected, declaredRange: '^9.0.0' }, selected), false)
  assert.equal(matchesSelectedFinding({ ...selected, severity: 'undeclared' }, selected), false)
  assert.equal(matchesSelectedFinding({ ...selected, origin: 'runtime' }, selected), false)
})


test('streams and hashes the pinned archive with host and size limits', async () => {
  const root = await mkdtemp(join(tmpdir(), 'xray-archive-test-'))
  try {
    const data = Buffer.from('registry fixture archive')
    const candidate = { url: 'https://registry.npmjs.org/example/-/example-1.0.0.tgz', sha512hex: createHash('sha512').update(data).digest('hex') }
    const mock = async (_url, options) => {
      assert.equal(options.redirect, 'error')
      return new Response(new ReadableStream({ start(controller) { controller.enqueue(data.subarray(0, 4)); controller.enqueue(data.subarray(4)); controller.close() } }))
    }
    await downloadVerifiedArchive(candidate, join(root, 'ok.tgz'), mock)
    await assert.rejects(downloadVerifiedArchive({ ...candidate, sha512hex: '0'.repeat(128) }, join(root, 'wrong.tgz'), mock), /archive_digest_mismatch/)
    await assert.rejects(downloadVerifiedArchive(candidate, join(root, 'large.tgz'), mock, 4), /archive_too_large/)
    const declaredTooLarge = async () => new Response(data, { headers: { 'content-length': '999' } })
    await assert.rejects(downloadVerifiedArchive(candidate, join(root, 'declared-large.tgz'), declaredTooLarge, 10), /archive_too_large/)
    await assert.rejects(downloadVerifiedArchive({ ...candidate, url: 'https://other.example/fixture.tgz' }, join(root, 'invalid.tgz'), mock), /invalid_archive_url/)
  } finally { await rm(root, { recursive: true, force: true }) }
})


test('requires package realpath to be outside the temporary project', () => {
  assert.equal(outsideProject('/tmp/project', '/tmp/store/pkg'), true)
  assert.equal(outsideProject('/tmp/project', '/tmp/project/node_modules/pkg'), false)
  assert.equal(outsideProject('/tmp/project', '/tmp/project'), false)
})
