#!/usr/bin/env node
// Downloads immutable npm tarballs; never installs or executes package code.
import { createHash } from 'node:crypto'
import { execFileSync, spawnSync } from 'node:child_process'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const binary = resolve(process.argv[2] ?? join(here, '../target/debug/xray'))
const cases = JSON.parse(await readFile(join(here, 'packages.json'), 'utf8'))
const temp = await mkdtemp(join(tmpdir(), 'xray-corpus-'))
let failed = false
try {
  for (const [index, test] of cases.entries()) {
    const response = await fetch(test.url, { signal: AbortSignal.timeout(60_000) })
    if (!response.ok) throw new Error(`${test.url}: HTTP ${response.status}`)
    const bytes = Buffer.from(await response.arrayBuffer())
    const digest = createHash('sha512').update(bytes).digest('hex')
    if (digest !== test.sha512) throw new Error(`${test.name}@${test.version}: integrity mismatch`)
    const archive = join(temp, `${index}.tgz`)
    await writeFile(archive, bytes)
    const directory = join(temp, `${index}`)
    execFileSync('mkdir', ['-p', directory])
    execFileSync('tar', ['-xzf', archive, '-C', directory])
    const result = spawnSync(binary, ['--json', '--package', join(directory, 'package')], { encoding: 'utf8', maxBuffer: 20 * 1024 * 1024 })
    if (result.error) throw result.error
    const expectedStatus = test.expectedIncomplete ? 2 : 0
    if (result.status !== expectedStatus) throw new Error(`${test.name}: xray exited ${result.status}, expected ${expectedStatus}: ${result.stderr}`)
    const output = JSON.parse(result.stdout)
    if (output.schemaVersion !== 1 || output.packagesScanned !== 1 || output.complete !== !test.expectedIncomplete || !Array.isArray(output.packages) || !Array.isArray(output.diagnostics)) {
      throw new Error(`${test.name}: invalid or unexpected JSON envelope: ${result.stdout}`)
    }
    const findings = output.packages.find((report) => report.package === `${test.name}@${test.version}`)?.findings ?? []
    for (const expected of test.expected) {
      if (!findings.some((actual) => Object.entries(expected).every(([key, value]) => actual[key] === value))) {
        console.error(`FAIL ${test.name}@${test.version}: missing ${JSON.stringify(expected)}`)
        failed = true
      } else {
        console.log(`PASS ${test.name}@${test.version}: ${expected.dependency}`)
      }
    }
    for (const dependency of test.notExpected ?? []) {
      if (findings.some((actual) => actual.dependency === dependency)) {
        console.error(`FAIL ${test.name}@${test.version}: unexpected finding ${dependency}`)
        failed = true
      }
    }
    if (test.maxFindings !== undefined && findings.length > test.maxFindings) {
      console.error(`FAIL ${test.name}@${test.version}: expected at most ${test.maxFindings} findings, got ${findings.length}: ${JSON.stringify(findings)}`)
      failed = true
    }
    for (const diagnostic of test.expectedDiagnostics ?? []) {
      if (!JSON.stringify(output.diagnostics).includes(diagnostic)) {
        console.error(`FAIL ${test.name}@${test.version}: missing diagnostic for ${diagnostic}`)
        failed = true
      }
    }
    const warningCount = output.diagnostics.length
    if (test.maxDiagnosticLines !== undefined && warningCount > test.maxDiagnosticLines) {
      console.error(`FAIL ${test.name}@${test.version}: expected at most ${test.maxDiagnosticLines} diagnostic lines, got ${warningCount}:\n${JSON.stringify(output.diagnostics)}`)
      failed = true
    }
    console.log(`${test.name}@${test.version}: ${findings.length} findings; ${warningCount} diagnostic lines`)
  }
} finally {
  await rm(temp, { recursive: true, force: true })
}
if (failed) process.exitCode = 1
