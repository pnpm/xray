import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { registryPin, safeEvidence, splitId } from './discover.mjs'

test('accepts exact public package ids, not filesystem paths', () => {
  assert.deepEqual(splitId('@scope/name@1.2.3-rc.1'), { name: '@scope/name', version: '1.2.3-rc.1' })
  for (const value of ['../private@1.0.0', '/tmp/private@1.0.0', 'name@file:../private', 'name@latest']) assert.equal(splitId(value), null)
})

test('filters absolute and escaping evidence paths', () => {
  assert.deepEqual(safeEvidence([
    { file: '/home/user/private.js', specifier: 'pkg' },
    { file: '../private.js', specifier: 'pkg' },
    { file: 'C:\\private.js', specifier: 'pkg' },
    { file: 'lib/index.js', specifier: '/home/user/private' },
    { file: 'lib/index.js', specifier: 'pkg/path', line: 2, column: 3, origin: 'runtime' },
  ]), [{ file: 'lib/index.js', specifier: 'pkg/path', line: 2, column: 3, origin: 'runtime' }])
})

test('requires registry HTTPS and a canonical sha512 digest', () => {
  const id = { name: 'fixture', version: '1.2.3' }
  const integrity = `sha512-${Buffer.alloc(64, 7).toString('base64')}`
  const metadata = { ...id, dist: { tarball: 'https://registry.npmjs.org/fixture/-/fixture-1.2.3.tgz', integrity } }
  assert.equal(registryPin(metadata, id).sha512hex, Buffer.alloc(64, 7).toString('hex'))
  assert.equal(registryPin({ ...metadata, version: '9.0.0' }, id), null)
  assert.equal(registryPin({ ...metadata, dist: { ...metadata.dist, tarball: 'https://other.example/fixture.tgz' } }, id), null)
  assert.equal(registryPin({ ...metadata, dist: { ...metadata.dist, integrity: 'sha512-bad' } }, id), null)
})
