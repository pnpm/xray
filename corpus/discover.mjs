#!/usr/bin/env node
// Mine *unverified* candidates from an existing installation. This reads public
// registry metadata only; it never executes installed packages or downloads tarballs.
import { spawnSync } from 'node:child_process'
import { lstat, opendir, readFile } from 'node:fs/promises'
import { dirname, isAbsolute, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const registry = 'https://registry.npmjs.org'
const here = dirname(fileURLToPath(import.meta.url))
const packageName = /^(?:@[a-z0-9][a-z0-9._~-]*\/)?[a-z0-9][a-z0-9._~-]*$/
const versionPattern = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/

export function splitId(id) {
  if (typeof id !== 'string') return null
  const index = id.lastIndexOf('@')
  if (index <= 0) return null
  const name = id.slice(0, index)
  const version = id.slice(index + 1)
  return packageName.test(name) && versionPattern.test(version) ? { name, version } : null
}

export function safeEvidence(items) {
  if (!Array.isArray(items)) return []
  return items.flatMap((item) => {
    if (!item || typeof item.file !== 'string' || typeof item.specifier !== 'string') return []
    const file = item.file.replaceAll('\\', '/')
    if (!file || isAbsolute(file) || /^[A-Za-z]:/.test(file) || file.split('/').some((part) => part === '..' || part === '')) return []
    const specifier = item.specifier
    if (specifier.startsWith('/') || /^[A-Za-z]:[\\/]/.test(specifier) || specifier.startsWith('file:') || specifier.split(/[\\/]/).includes('..')) return []
    const evidence = { file, specifier }
    if (Number.isSafeInteger(item.line) && item.line > 0) evidence.line = item.line
    if (Number.isSafeInteger(item.column) && item.column > 0) evidence.column = item.column
    if (['runtime', 'types', 'both'].includes(item.origin)) evidence.origin = item.origin
    return [evidence]
  })
}

export function registryPin(metadata, expected) {
  if (metadata?.name !== expected.name || metadata.version !== expected.version) return null
  const url = metadata.dist?.tarball
  let parsed
  try { parsed = new URL(url) } catch { return null }
  if (parsed.protocol !== 'https:' || parsed.hostname !== 'registry.npmjs.org' || parsed.port || parsed.username || parsed.password || parsed.search || parsed.hash || !parsed.pathname.endsWith('.tgz')) return null
  const integrity = typeof metadata.dist?.integrity === 'string' ? metadata.dist.integrity.split(/\s+/).find((part) => part.startsWith('sha512-')) : undefined
  if (!integrity || !/^sha512-[A-Za-z0-9+/]{86}==$/.test(integrity)) return null
  const bytes = Buffer.from(integrity.slice(7), 'base64')
  if (bytes.length !== 64 || bytes.toString('base64') !== integrity.slice(7)) return null
  return { url: parsed.href, sha512hex: bytes.toString('hex') }
}

// Exclude every manifest in the checkout, not just lockfile importers. This also
// avoids exposing local fixture names when a workspace happens to share a public ID.
async function localNames(root) {
  const names = new Set()
  async function visit(dir) {
    let entries
    try { entries = await opendir(dir) } catch (error) { if (error.code === 'ENOENT') return; throw error }
    for await (const entry of entries) {
      if (entry.isSymbolicLink()) continue
      const target = join(dir, entry.name)
      if (entry.isDirectory()) {
        if (['node_modules', '.git', '.pnpm', '.pnpm-store'].includes(entry.name)) continue
        await visit(target)
      } else if (entry.isFile() && entry.name === 'package.json') {
        try {
          const manifest = JSON.parse(await readFile(target, 'utf8'))
          if (typeof manifest.name === 'string') names.add(manifest.name)
        } catch (error) {
          if (error instanceof SyntaxError || error.code === 'ENOENT') continue
          throw error
        }
      }
    }
  }
  await visit(root)
  return names
}

async function getPin(id) {
  const endpoint = `${registry}/${encodeURIComponent(id.name)}/${encodeURIComponent(id.version)}`
  try {
    const response = await fetch(endpoint, { redirect: 'error', signal: AbortSignal.timeout(10_000), headers: { accept: 'application/json' } })
    if (!response.ok) return null
    return registryPin(await response.json(), id)
  } catch { return null }
}

export async function discover(project, binary) {
  const root = resolve(project)
  if (!(await lstat(root)).isDirectory()) throw new Error('project must be a directory')
  const names = await localNames(root)
  const scan = spawnSync(resolve(binary), [root, '--json'], { encoding: 'utf8', maxBuffer: 128 * 1024 * 1024 })
  if (scan.error || ![0, 2].includes(scan.status)) throw new Error('xray scan failed; check the binary and project installation')
  let result
  try { result = JSON.parse(scan.stdout) } catch { throw new Error('xray returned invalid JSON') }
  if (result.schemaVersion !== 1 || !Array.isArray(result.packages) || !Array.isArray(result.diagnostics) || typeof result.complete !== 'boolean') throw new Error('xray returned an unsupported JSON envelope')
  const found = new Map()
  for (const report of result.packages) {
    const id = splitId(report.package)
    if (!id || names.has(id.name) || !Array.isArray(report.findings)) continue
    const findings = report.findings.flatMap((finding) => {
      if (!finding || typeof finding.dependency !== 'string' || names.has(finding.dependency) || !packageName.test(finding.dependency) || !['runtime', 'types', 'both'].includes(finding.origin) || !['devDependency', 'undeclared'].includes(finding.severity)) return []
      const evidence = safeEvidence(finding.evidence).filter((item) => {
        const raw = item.specifier.replace(/^~/, '')
        const parts = raw.split('/')
        const name = raw.startsWith('@') ? parts.slice(0, 2).join('/') : parts[0]
        return !names.has(name)
      })
      if (!evidence.length) return []
      const observed = { dependency: finding.dependency, severity: finding.severity, origin: finding.origin, evidence }
      // Preserve simple public semver ranges for reproducible consumer installs.
      if (typeof finding.declaredRange === 'string' && /^[~^]?[0-9]+(?:\.[0-9]+){0,2}(?:-[0-9A-Za-z.-]+)?$/.test(finding.declaredRange)) observed.declaredRange = finding.declaredRange
      return [observed]
    })
    if (!findings.length) continue
    const key = `${id.name}@${id.version}`
    if (found.has(key)) found.get(key).observedFindings.push(...findings)
    else found.set(key, { ...id, observedFindings: findings })
  }
  const candidates = [...found.values()]
  let cursor = 0
  const verified = []
  await Promise.all(Array.from({ length: Math.min(4, candidates.length) }, async () => {
    while (cursor < candidates.length) {
      const candidate = candidates[cursor++]
      const pin = await getPin(candidate)
      if (pin) verified.push({ ...candidate, ...pin })
    }
  }))
  // Put author-declared type leaks first; this is triage order, not a verdict.
  const score = (candidate) => candidate.observedFindings.reduce((sum, f) => sum + (f.severity === 'devDependency' ? 2 : 0) + (f.origin === 'types' ? 1 : 0), 0)
  verified.sort((a, b) => score(b) - score(a) || a.name.localeCompare(b.name) || a.version.localeCompare(b.version))
  return { schemaVersion: 1, status: 'unverified', scanComplete: result.complete, diagnosticsCount: result.diagnostics.length, candidates: verified }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const project = process.argv[2]
    if (!project || process.argv.length > 4) throw new Error('usage: node corpus/discover.mjs PROJECT [XRAY_BINARY]')
    const binary = process.argv[3] ?? join(here, '../target/debug/xray')
    console.log(JSON.stringify(await discover(project, binary), null, 2))
  } catch (error) {
    // Avoid printing local paths or raw scanner output.
    console.error(`discovery failed: ${error instanceof Error && error.message.startsWith('usage:') ? error.message : 'cannot scan project or read registry metadata'}`)
    process.exitCode = 1
  }
}
