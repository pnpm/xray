// Generate the Cargo.lock runtime-license union without network access.
// cargo-about's SPDX fallback has no copyright notices. Reviewed upstream
// source files replace those fallbacks; fail closed when the set changes.
import { createHash } from 'node:crypto'
import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const about = process.env.CARGO_ABOUT || 'cargo-about'
function run(args) {
  const result = spawnSync(about, args, { cwd: root, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 })
  if (result.error || result.status !== 0) throw new Error(`cargo-about failed: ${result.error?.message || result.stderr}`)
  return result.stdout
}
if (run(['--version']).trim() !== 'cargo-about 0.9.3') throw new Error('cargo-about 0.9.3 is required')
const data = JSON.parse(run(['generate', '--frozen', '--fail', '--format', 'json']))
const overrides = JSON.parse(readFileSync(join(root, 'licenses/rust-overrides.json'), 'utf8'))
const usedOverrides = new Set()
const sections = []
for (const license of data.licenses) {
  const names = license.used_by.map(item => `${item.crate.name}@${item.crate.version}`).sort()
  if (license.source_path != null) {
    sections.push({ label: `${license.name} (${license.id})`, names, text: license.text })
    continue
  }
  for (const name of names) {
    const override = overrides[name]
    if (!override) throw new Error(`unreviewed license fallback: ${name} (${license.id})`)
    if (license.id !== (override.generic ? 'Apache-2.0' : 'MIT')) throw new Error(`unexpected license for fallback: ${name}`)
    usedOverrides.add(name)
    let text = license.text
    if (override.generic) {
      if (name !== 'yaml-edit@0.3.5' || license.id !== 'Apache-2.0') throw new Error(`invalid generic exception: ${name}`)
    } else {
      if (!/^https:\/\/raw\.githubusercontent\.com\/[^/]+\/[^/]+\/[0-9a-f]{40}\//.test(override.source) || !/^[0-9a-f]{64}\.txt$/.test(override.file)) throw new Error(`invalid pinned license source: ${name}`)
      const bytes = readFileSync(join(root, 'licenses/rust-overrides', override.file))
      const digest = createHash('sha256').update(bytes).digest('hex')
      if (digest !== override.sha256 || `${digest}.txt` !== override.file) throw new Error(`license source hash mismatch: ${name}`)
      text = bytes.toString('utf8')
      if (override.supplement_spdx) {
        if (license.id !== 'MIT') throw new Error(`unexpected supplemental license: ${name}`)
        // These upstream files state the MIT choice and attribution (if any),
        // but do not reproduce its terms. Do not invent a copyright holder.
        text += `\n\n${license.text.replace(/^Copyright \(c\) <year> <copyright holders>\s*$/m, '').trim()}\n`
      }
    }
    sections.push({ label: `${license.name} (${license.id})`, names: [name], text })
  }
}
for (const name of Object.keys(overrides)) if (!usedOverrides.has(name)) throw new Error(`stale license override: ${name}`)
sections.sort((a, b) => a.names.join(',') < b.names.join(',') ? -1 : a.names.join(',') > b.names.join(',') ? 1 : 0)
const body = [
  'Rust crates',
  '',
  'The following notices cover runtime dependencies across all native release',
  'targets. Build-only and test-only dependencies are excluded.',
  '',
  ...sections.flatMap(section => [
    '------------------------------------------------------------------------------',
    section.label,
    '',
    ...section.names.map(name => `- ${name}`),
    '',
    section.text.replaceAll('\r\n', '\n').replace(/[ \t]+$/gm, '').trim(),
    '',
  ]),
].join('\n')
const notice = join(root, 'THIRD_PARTY_NOTICES')
const original = readFileSync(notice, 'utf8')
const startMarker = '\nRust crates\n'
const separator = '\n==============================================================================\n'
const start = original.indexOf(startMarker)
const end = original.indexOf(separator, start + startMarker.length)
if (start < 0 || end < 0 || original.indexOf(startMarker, start + 1) !== -1) throw new Error('invalid Rust notices markers')
const result = original.slice(0, start + 1) + body.trimEnd() + '\n' + original.slice(end)
if (process.argv.includes('--check')) {
  if (original !== result) throw new Error('Rust third-party notices are stale; run node scripts/generate-rust-notices.mjs')
  console.log('Rust third-party notices match Cargo.lock')
} else {
  writeFileSync(notice, result)
  console.log('Updated Rust third-party notices')
}
