import { strict as assert } from 'node:assert'
import { readFile } from 'node:fs/promises'
import { test } from 'node:test'
import { REGISTRY_CUTOFF, REGISTRY_PINS, auditedDependencies, cutoffForPins, minimumReleaseAge, registryWindow, remainingTime, RUN_WINDOW_MS, validateRegistryPins } from './registry-cutoff.mjs'
import { candidatePins, fetchPublishedPin, splitExactPin } from './update-registry-pins.mjs'

const integrity = `sha512-${Buffer.alloc(64, 1).toString('base64')}`
const pin = { name: 'example-package', version: '1.2.3', publishedAt: '2026-11-02T12:34:56.123Z', integrity }

test('the fixed cutoff is derived from committed metadata, never the current clock', () => {
  assert.equal(REGISTRY_CUTOFF, '2026-10-31T00:00:00.000Z')
  assert.equal(cutoffForPins([{ ...pin, publishedAt: '2020-01-01T00:00:00.000Z' }]), REGISTRY_CUTOFF)
  const nextCutoff = Date.parse(cutoffForPins([pin]))
  assert.equal(new Date(nextCutoff).toISOString(), '2026-11-02T13:21:00.000Z')
  for (const offset of [0, 1, 59_999]) {
    const start = nextCutoff + 86_400_000 + offset
    const age = Math.ceil((start + RUN_WINDOW_MS - nextCutoff) / 60_000)
    assert.ok(start - age * 60_000 >= Date.parse(pin.publishedAt), 'pin must be eligible from the start despite minute rounding')
    assert.ok(start + RUN_WINDOW_MS - age * 60_000 <= nextCutoff, 'cutoff must remain bounded at the deadline')
  }
  assert.equal(cutoffForPins([{ ...pin, publishedAt: '2026-11-02T12:35:00.000Z' }]), '2026-11-02T13:21:00.000Z')
  assert.equal(minimumReleaseAge(Date.parse(REGISTRY_CUTOFF) + 60_001), 2)
  assert.throws(() => minimumReleaseAge(Date.parse(REGISTRY_CUTOFF)), /future/)
  const now = Date.parse(REGISTRY_CUTOFF) + 60_001
  const window = registryWindow(now)
  assert.equal(window.deadline, now + RUN_WINDOW_MS)
  assert.ok(window.deadline - window.releaseAge * 60_000 <= Date.parse(REGISTRY_CUTOFF))
  assert.equal(remainingTime(window.deadline, 500, window.deadline - 100), 100)
  assert.throws(() => remainingTime(window.deadline, 500, window.deadline), /expired/)
  assert.throws(() => registryWindow(Date.parse(REGISTRY_CUTOFF)), /future/)
})

test('metadata must contain unique exact identities, canonical timestamps and SHA-512', () => {
  assert.deepEqual(validateRegistryPins({ schemaVersion: 1, pins: [pin] }), [pin])
  for (const invalid of [
    { ...pin, name: '../private' }, { ...pin, version: '^1.2.3' }, { ...pin, version: 'latest' },
    { ...pin, publishedAt: 'tomorrow' }, { ...pin, publishedAt: '2026-02-30T00:00:00.000Z' },
    { ...pin, integrity: 'sha512-not-a-digest' }, { ...pin, integrity: undefined },
  ]) assert.throws(() => validateRegistryPins({ schemaVersion: 1, pins: [invalid] }))
  assert.throws(() => validateRegistryPins({ schemaVersion: 1, pins: [pin, pin] }), /duplicate/)
  assert.throws(() => validateRegistryPins({ schemaVersion: 2, pins: [pin] }))
})

test('fixture dependency maps reject unreviewed pins and floating ranges', () => {
  const dependency = { '@types/node': '24.19.2', typescript: '5.9.3' }
  assert.equal(auditedDependencies(dependency), dependency)
  assert.throws(() => auditedDependencies({ typescript: '^5.9.3' }), /unaudited/)
  assert.throws(() => auditedDependencies({ typescript: '999.0.0' }), /unaudited/)
  assert.throws(() => auditedDependencies({ 'new-dependency': '1.0.0' }), /unaudited/)
})

test('all checked-in install fixture dependency literals are guarded and audited', async () => {
  const used = new Set()
  for (const file of ['install-test.mjs', 'verify-loop.mjs']) {
    const source = await readFile(new URL(file, import.meta.url), 'utf8')
    assert.doesNotMatch(source, /\bdependencies:\s*\{/)
    const maps = [...source.matchAll(/\bdependencies:\s*auditedDependencies\(\{([^{}]*)\}\)/g)]
    assert.ok(maps.length > 0, `${file}: no guarded dependency maps`)
    for (const [, map] of maps) {
      const entries = map.split(',').map((item) => item.trim()).filter(Boolean)
      for (const entry of entries) {
        const match = /^(?:'([^']+)'|([A-Za-z0-9_-]+)):\s*(?:'([^']+)'|(nodeTypes))$/.exec(entry)
        assert.ok(match, `${file}: unrecognized dependency entry ${entry}`)
        const name = match[1] ?? match[2]
        if (match[4]) {
          assert.equal(name, '@types/node')
          assert.match(source, /\['22\.14\.1', true\], \['24\.19\.2', false\]/)
          for (const version of ['22.14.1', '24.19.2']) used.add(`${name}@${version}`)
        } else used.add(`${name}@${match[3]}`)
      }
    }
  }
  const recorded = new Set(REGISTRY_PINS.map(({ name, version }) => `${name}@${version}`))
  assert.deepEqual([...used].sort(), [...recorded].sort(), 'registry metadata must cover exactly the current direct fixture pins')
})

test('explicit metadata update emits candidates but does not mutate the fixed cutoff', async () => {
  assert.deepEqual(splitExactPin('@scope/example@1.2.3'), { name: '@scope/example', version: '1.2.3' })
  for (const bad of ['@scope/example', 'example@latest', 'example@^1.0.0', '../private@1.0.0']) assert.throws(() => splitExactPin(bad))
  const fetcher = async (url, options) => {
    assert.equal(url, 'https://registry.npmjs.org/example-package')
    assert.equal(options.redirect, 'error')
    return { ok: true, json: async () => ({ name: pin.name, versions: { [pin.version]: { name: pin.name, version: pin.version, dist: { integrity } } }, time: { [pin.version]: pin.publishedAt } }) }
  }
  const candidate = await candidatePins(['example-package@1.2.3'], fetcher)
  assert.deepEqual(candidate.pins.find((entry) => entry.name === pin.name), pin)
  assert.equal(cutoffForPins(candidate.pins), '2026-11-02T13:21:00.000Z')
  assert.equal(REGISTRY_CUTOFF, '2026-10-31T00:00:00.000Z')
  await assert.rejects(() => candidatePins(['typescript@5.9.3'], fetcher), /already exists/)
  await assert.rejects(() => fetchPublishedPin({ name: pin.name, version: pin.version }, async () => ({ ok: true, json: async () => ({ name: 'different' }) })), /identity mismatch/)
  await assert.rejects(() => fetchPublishedPin({ name: pin.name, version: pin.version }, async () => ({ ok: false, status: 503 })), /HTTP 503/)
})

test('pins shared with the tarball corpus retain the same SHA-512', async () => {
  const archives = JSON.parse(await readFile(new URL('./packages.json', import.meta.url), 'utf8'))
  const pins = new Map(REGISTRY_PINS.map((pin) => [`${pin.name}@${pin.version}`, pin]))
  let compared = 0
  for (const archive of archives) {
    const pin = pins.get(`${archive.name}@${archive.version}`)
    if (!pin) continue
    assert.equal(Buffer.from(pin.integrity.slice(7), 'base64').toString('hex'), archive.sha512, `${archive.name}@${archive.version}`)
    compared++
  }
  assert.ok(compared > 0, 'expected archive pins shared with the install corpus')
})
