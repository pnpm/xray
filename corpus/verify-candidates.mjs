#!/usr/bin/env node
// Validate a bounded set of unverified declarations; never run package code.
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdtemp, open, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { registryPin, splitId } from './discover.mjs'

const here = dirname(fileURLToPath(import.meta.url))
const rangePattern = /^[~^]?[0-9]+(?:\.[0-9]+){0,2}(?:-[0-9A-Za-z.-]+)?$/
const namePattern = /^(?:@[a-z0-9][a-z0-9._~-]*\/)?[a-z0-9][a-z0-9._~-]*$/
const compiler = '5.9.3'
const nodeTypes = '22.14.1'

export function eligibleFinding(finding) {
  return Boolean(finding && finding.severity === 'devDependency' && finding.origin === 'types' &&
    typeof finding.dependency === 'string' && namePattern.test(finding.dependency) &&
    typeof finding.declaredRange === 'string' && rangePattern.test(finding.declaredRange))
}

export function matchesSelectedFinding(item, selected) {
  return Boolean(item && selected && item.dependency === selected.dependency && item.origin === selected.origin &&
    item.severity === selected.severity && item.declaredRange === selected.declaredRange)
}

export function expectedMissing(output, dependency) {
  const names = [dependency]
  if (dependency.startsWith('@types/')) {
    const basename = dependency.slice(7)
    names.push(basename.includes('__') ? `@${basename.replace('__', '/')}` : basename)
  }
  return output.split(/\r?\n/).some((line) => /\bTS(?:2307|2688)\b/.test(line) && names.some((name) => line.includes(`'${name}'`) || line.includes(`"${name}"`) || line.includes(`'${name}/`) || line.includes(`"${name}/`)))
}

export async function expectedFromTarget(output, dependency, temp, targetName) {
  let target
  try { target = await realpath(join(temp, 'node_modules', targetName)) } catch { return false }
  for (const line of output.split(/\r?\n/)) {
    if (!expectedMissing(line, dependency)) continue
    const match = /^(.*?)\(\d+,\d+\):\s*error\s+TS(?:2307|2688)\b/.exec(line)
    if (!match) continue
    try {
      const file = await realpath(resolve(temp, match[1]))
      const child = relative(target, file)
      if (child && child !== '..' && !child.startsWith('../') && !child.startsWith('/') && !child.split('/').includes('node_modules')) return true
    } catch { /* An unverifiable diagnostic cannot establish this finding. */ }
  }
  return false
}

function command(executable, args, cwd) {
  const result = spawnSync(executable, args, { cwd, encoding: 'utf8', timeout: 120_000, maxBuffer: 16 * 1024 * 1024 })
  return { status: result.status, error: Boolean(result.error), stdout: result.stdout ?? '', stderr: result.stderr ?? '' }
}

async function registryMetadata(name, version) {
  try {
    const response = await fetch(`https://registry.npmjs.org/${encodeURIComponent(name)}/${encodeURIComponent(version)}`, {
      redirect: 'error', signal: AbortSignal.timeout(10_000), headers: { accept: 'application/json' },
    })
    if (!response.ok) return null
    return await response.json()
  } catch { return null }
}

async function registryMatches(candidate) {
  const metadata = await registryMetadata(candidate.name, candidate.version)
  const pin = registryPin(metadata, candidate)
  return Boolean(pin && pin.url === candidate.url && pin.sha512hex === candidate.sha512hex)
}

// Stream and hash before pnpm sees the archive; never trust a scoped registry
// override or a candidate-supplied download host. The verifier's provider and
// compiler pins remain registry metadata rather than independently hashed files.
export async function downloadVerifiedArchive(candidate, destination, fetchImpl = fetch, maxBytes = 64 * 1024 * 1024) {
  let url
  try { url = new URL(candidate.url) } catch { throw new Error('invalid_archive_url') }
  if (url.protocol !== 'https:' || url.hostname !== 'registry.npmjs.org' || url.port || url.username || url.password || url.search || url.hash || !url.pathname.endsWith('.tgz') || !/^[0-9a-f]{128}$/.test(candidate.sha512hex)) throw new Error('invalid_archive_url')
  const response = await fetchImpl(url.href, { redirect: 'error', signal: AbortSignal.timeout(30_000) })
  if (!response.ok || !response.body) throw new Error('archive_download_failed')
  const length = Number(response.headers.get('content-length'))
  if (Number.isFinite(length) && length > maxBytes) {
    await response.body.cancel()
    throw new Error('archive_too_large')
  }
  const file = await open(destination, 'wx')
  const digest = createHash('sha512')
  let size = 0
  try {
    for await (const chunk of response.body) {
      size += chunk.length
      if (size > maxBytes) throw new Error('archive_too_large')
      digest.update(chunk)
      await file.writeFile(chunk)
    }
  } finally { await file.close() }
  if (digest.digest('hex') !== candidate.sha512hex) throw new Error('archive_digest_mismatch')
}

export function outsideProject(project, target) {
  const location = relative(project, target)
  return Boolean(location && (location === '..' || location.startsWith('../')))
}

function diagnosisCodes(output) {
  return [...new Set([...output.matchAll(/\bTS\d{4,5}\b/g)].map((match) => match[0]))].sort()
}

export function readScan(result) {
  if (result.error || ![0, 2].includes(result.status)) return null
  try {
    const scan = JSON.parse(result.stdout)
    if (scan.schemaVersion !== 1 || !Array.isArray(scan.packages) || !Array.isArray(scan.diagnostics) || typeof scan.complete !== 'boolean' || result.status !== (scan.complete ? 0 : 2)) return null
    return scan
  } catch { return null }
}

async function verifyOne(candidate, binary) {
  const identity = { name: candidate.name, version: candidate.version, url: candidate.url, sha512hex: candidate.sha512hex }
  const finding = candidate.observedFindings.find(eligibleFinding)
  if (!finding) return { ...identity, status: 'skipped', reason: 'no_supported_type_finding' }
  if (finding.dependency === candidate.name || ['typescript', '@types/node'].includes(finding.dependency) || ['typescript', '@types/node'].includes(candidate.name)) {
    return { ...identity, status: 'skipped', reason: 'consumer_dependency_conflict' }
  }
  const publicProvider = await registryMetadata(finding.dependency, 'latest')
  if (!publicProvider || publicProvider.name !== finding.dependency) return { ...identity, status: 'skipped', reason: 'provider_not_public_registry' }
  const details = { ...identity, dependency: finding.dependency, declaredRange: finding.declaredRange }
  const temp = await mkdtemp(join(tmpdir(), 'xray-candidate-'))
  try {
    try { await downloadVerifiedArchive(candidate, join(temp, 'target.tgz')) } catch (error) {
      const known = ['invalid_archive_url', 'archive_too_large', 'archive_digest_mismatch']
      return { ...details, status: 'blocked', reason: known.includes(error?.message) ? error.message : 'archive_download_failed' }
    }
    details.targetArchive = 'sha512_verified'
    const deps = { [candidate.name]: 'file:./target.tgz', [finding.dependency]: finding.declaredRange, typescript: compiler, '@types/node': nodeTypes }
    await writeFile(join(temp, 'package.json'), JSON.stringify({ private: true, type: 'module', dependencies: deps }, null, 2))
    await writeFile(join(temp, 'pnpm-workspace.yaml'), 'hoist: false\nenableGlobalVirtualStore: true\n')
    await writeFile(join(temp, 'index.ts'), `import * as target from ${JSON.stringify(candidate.name)};\nvoid target;\n`)
    await writeFile(join(temp, 'tsconfig.json'), JSON.stringify({ compilerOptions: { module: 'nodenext', moduleResolution: 'nodenext', target: 'esnext', noEmit: true, skipLibCheck: false }, include: ['index.ts'] }))
    const install = command('pnpm', ['install', '--ignore-scripts', '--registry=https://registry.npmjs.org/'], temp)
    if (install.error || install.status !== 0) return { ...details, status: 'blocked', reason: 'install_failed' }
    let installedTarget
    try { installedTarget = JSON.parse(await readFile(join(temp, 'node_modules', candidate.name, 'package.json'), 'utf8')) } catch {
      return { ...details, status: 'blocked', reason: 'target_manifest_unavailable' }
    }
    if (installedTarget.name !== candidate.name || installedTarget.version !== candidate.version) return { ...details, status: 'blocked', reason: 'target_identity_mismatch' }
    let targetPath
    try { targetPath = await realpath(join(temp, 'node_modules', candidate.name)) } catch {
      return { ...details, status: 'blocked', reason: 'target_manifest_unavailable' }
    }
    if (!outsideProject(await realpath(temp), targetPath)) return { ...details, status: 'blocked', reason: 'layout_not_global' }
    details.layout = 'outside_project'
    let installedCompiler
    try { installedCompiler = JSON.parse(await readFile(join(temp, 'node_modules/typescript/package.json'), 'utf8')) } catch {
      return { ...details, status: 'blocked', reason: 'compiler_manifest_unavailable' }
    }
    if (installedCompiler.name !== 'typescript' || installedCompiler.version !== compiler) return { ...details, status: 'blocked', reason: 'compiler_identity_mismatch' }
    let installedProvider
    try { installedProvider = JSON.parse(await readFile(join(temp, 'node_modules', finding.dependency, 'package.json'), 'utf8')) } catch {
      return { ...details, status: 'blocked', reason: 'provider_manifest_unavailable' }
    }
    const providerId = splitId(`${installedProvider.name}@${installedProvider.version}`)
    if (!providerId || providerId.name !== finding.dependency) return { ...details, status: 'blocked', reason: 'provider_identity_mismatch' }
    const providerPin = registryPin(await registryMetadata(providerId.name, providerId.version), providerId)
    if (!providerPin) return { ...details, status: 'blocked', reason: 'provider_pin_unavailable' }
    details.provider = { ...providerId, ...providerPin }
    const tsc = join(temp, 'node_modules/typescript/bin/tsc')
    const before = command(process.execPath, [tsc, '--noEmit'], temp)
    const beforeOutput = before.stdout + before.stderr
    if (before.error) return { ...details, status: 'blocked', reason: 'typecheck_failed_to_run' }
    if (!await expectedFromTarget(beforeOutput, finding.dependency, temp, candidate.name)) {
      return { ...details, status: 'skipped', reason: 'expected_missing_error_not_observed', beforeCodes: diagnosisCodes(beforeOutput) }
    }
    const initialScan = readScan(command(binary, ['--json'], temp))
    if (!initialScan || !initialScan.complete) return { ...details, status: 'blocked', reason: 'initial_scan_incomplete', beforeCodes: diagnosisCodes(beforeOutput) }
    const target = initialScan.packages.find((pkg) => pkg.package === `${candidate.name}@${candidate.version}`)
    if (!target?.findings?.some((item) => matchesSelectedFinding(item, finding))) return { ...details, status: 'skipped', reason: 'initial_scan_finding_not_observed', beforeCodes: diagnosisCodes(beforeOutput) }
    const repair = command(binary, ['--fix'], temp)
    if (repair.error || repair.status !== 0) return { ...details, status: 'blocked', reason: 'repair_failed', beforeCodes: diagnosisCodes(beforeOutput) }
    const reinstall = command('pnpm', ['install', '--no-frozen-lockfile', '--ignore-scripts', '--registry=https://registry.npmjs.org/'], temp)
    if (reinstall.error || reinstall.status !== 0) return { ...details, status: 'blocked', reason: 'reinstall_failed', beforeCodes: diagnosisCodes(beforeOutput) }
    try {
      const afterTarget = JSON.parse(await readFile(join(temp, 'node_modules', candidate.name, 'package.json'), 'utf8'))
      const afterCompiler = JSON.parse(await readFile(join(temp, 'node_modules/typescript/package.json'), 'utf8'))
      const afterPath = await realpath(join(temp, 'node_modules', candidate.name))
      if (afterTarget.name !== candidate.name || afterTarget.version !== candidate.version || afterCompiler.name !== 'typescript' || afterCompiler.version !== compiler || !outsideProject(await realpath(temp), afterPath)) {
        return { ...details, status: 'blocked', reason: 'post_install_identity_or_layout_changed', beforeCodes: diagnosisCodes(beforeOutput) }
      }
    } catch { return { ...details, status: 'blocked', reason: 'post_install_identity_or_layout_changed', beforeCodes: diagnosisCodes(beforeOutput) } }
    const after = command(process.execPath, [tsc, '--noEmit'], temp)
    const afterOutput = after.stdout + after.stderr
    if (after.error || after.status !== 0) return { ...details, status: 'blocked', reason: 'typecheck_still_fails', beforeCodes: diagnosisCodes(beforeOutput), afterCodes: diagnosisCodes(afterOutput) }
    const finalScan = readScan(command(binary, ['--json'], temp))
    if (!finalScan || !finalScan.complete) return { ...details, status: 'blocked', reason: 'final_scan_incomplete', beforeCodes: diagnosisCodes(beforeOutput) }
    const remaining = finalScan.packages.find((pkg) => pkg.package === `${candidate.name}@${candidate.version}`)
    if (remaining?.findings?.some((item) => item.dependency === finding.dependency)) return { ...details, status: 'blocked', reason: 'finding_still_reported', beforeCodes: diagnosisCodes(beforeOutput) }
    return { ...details, status: 'confirmed', evidence: 'missing_type_module_resolved_after_generated_extension', beforeCodes: diagnosisCodes(beforeOutput), afterCodes: [], providerPin: 'metadata_only', compiler, nodeTypes }
  } finally {
    await rm(temp, { recursive: true, force: true })
  }
}

export async function verifyCandidates(input, binary, limit) {
  if (process.platform !== 'linux') throw new Error('verification currently supports Linux only')
  if (input?.schemaVersion !== 1 || input.status !== 'unverified' || !Array.isArray(input.candidates)) throw new Error('invalid discovery envelope')
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 25) throw new Error('limit must be between 1 and 25')
  const results = []
  let examined = 0
  let registryUnverified = 0
  const supported = (candidate) => Array.isArray(candidate?.observedFindings) && candidate.observedFindings.some(eligibleFinding)
  const ordered = input.candidates.map((candidate, index) => ({ candidate, index }))
    .sort((a, b) => Number(supported(b.candidate)) - Number(supported(a.candidate)) || a.index - b.index)
  for (const { candidate } of ordered) {
    if (examined >= limit) break
    const id = splitId(`${candidate?.name}@${candidate?.version}`)
    if (!id || id.name !== candidate.name || id.version !== candidate.version || !Array.isArray(candidate.observedFindings)) continue
    // Do not echo input identities unless exact public registry metadata agrees.
    examined++
    if (!await registryMatches(candidate)) { registryUnverified++; continue }
    results.push(await verifyOne(candidate, resolve(binary)))
  }
  return { schemaVersion: 1, status: 'verification_results', providerPins: 'metadata_only', registryUnverified, results }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const args = process.argv.slice(2)
    const input = args.shift()
    if (!input) throw new Error('usage')
    let limit = 5
    let binary = join(here, '../target/debug/xray')
    while (args.length) {
      const flag = args.shift()
      if (flag === '--limit' && args.length) limit = Number(args.shift())
      else if (flag === '--xray' && args.length) binary = args.shift()
      else throw new Error('usage')
    }
    const document = JSON.parse(await readFile(resolve(input), 'utf8'))
    console.log(JSON.stringify(await verifyCandidates(document, binary, limit), null, 2))
  } catch {
    console.error('verification failed: check the input, binary, Linux environment, and --limit (1–25)')
    process.exitCode = 1
  }
}
