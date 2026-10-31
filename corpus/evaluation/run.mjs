#!/usr/bin/env node
// Evaluate against the frozen, manually reviewed labels. Never installs or runs package code.
import { createHash } from 'node:crypto'
import { execFileSync, spawnSync } from 'node:child_process'
import { mkdtemp, mkdir, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { downloadVerifiedArchive } from '../verify-candidates.mjs'

const here = dirname(fileURLToPath(import.meta.url))
const frozenHash = 'c51c93ec127e98b4b51941863376e21261ed953ad8bcf215ab119bc79fd2cdc6'
const binary = resolve(process.argv[2] ?? join(here, '../../target/debug/xray'))
const bytes = await readFile(join(here, 'frozen.json'))
if (createHash('sha256').update(bytes).digest('hex') !== frozenHash) throw new Error('frozen evaluation manifest hash does not match the recorded freeze')
const manifest = JSON.parse(bytes)
const temp = await mkdtemp(join(tmpdir(), 'xray-heldout-'))
const totals = { truePositive: 0, falsePositive: 0, falseNegative: 0, unknownFindings: 0, incompletePackages: 0 }
const results = []
try {
  for (const [index, test] of manifest.cases.entries()) {
    const directory = join(temp, String(index))
    await mkdir(directory)
    const file = join(temp, `${index}.tgz`)
    await downloadVerifiedArchive({ url: test.url, sha512hex: test.sha512 }, file)
    execFileSync('tar', ['-xzf', file, '-C', directory])
    const identity = JSON.parse(await readFile(join(directory, 'package/package.json'), 'utf8'))
    if (identity.name !== test.name || identity.version !== test.version) throw new Error(`${test.name}: archive identity mismatch`)
    const result = spawnSync(binary, ['--json', '--package', join(directory, 'package')], { encoding: 'utf8', timeout: 60_000, maxBuffer: 20 * 1024 * 1024 })
    if (result.error) throw result.error
    if (![0, 2].includes(result.status)) throw new Error(`${test.name}: xray exited ${result.status}: ${result.stderr}`)
    const scan = JSON.parse(result.stdout)
    if (scan.schemaVersion !== 1 || scan.packagesScanned !== 1 || scan.complete !== (result.status === 0) || !Array.isArray(scan.packages) || !Array.isArray(scan.diagnostics)) throw new Error(`${test.name}: inconsistent scan envelope`)
    const observed = [...new Set((scan.packages.find((p) => p.package === `${test.name}@${test.version}`)?.findings ?? []).map((finding) => finding.dependency))].sort()
    const expected = [...test.expected].sort()
    if (!scan.complete) {
      totals.incompletePackages++
      totals.unknownFindings += observed.length
      results.push({ package: `${test.name}@${test.version}`, status: 'incomplete', expected, observed, diagnostics: scan.diagnostics })
      continue
    }
    const truePositive = expected.filter((name) => observed.includes(name))
    const falseNegative = expected.filter((name) => !observed.includes(name))
    const extra = observed.filter((name) => !expected.includes(name))
    const falsePositive = test.clean ? extra : []
    const unknown = test.clean ? [] : extra
    totals.truePositive += truePositive.length
    totals.falseNegative += falseNegative.length
    totals.falsePositive += falsePositive.length
    totals.unknownFindings += unknown.length
    results.push({ package: `${test.name}@${test.version}`, status: 'complete', truePositive, falseNegative, falsePositive, unknown })
  }
} finally {
  await rm(temp, { recursive: true, force: true })
}
const ratio = (numerator, denominator) => denominator ? numerator / denominator : null
console.log(JSON.stringify({ schemaVersion: 1, frozenSha256: frozenHash, scope: 'manually reviewed labels only; unknown findings are not scored', totals, precision: ratio(totals.truePositive, totals.truePositive + totals.falsePositive), recall: ratio(totals.truePositive, totals.truePositive + totals.falseNegative), results }, null, 2))
if (totals.falsePositive || totals.falseNegative || totals.incompletePackages) process.exitCode = 1
