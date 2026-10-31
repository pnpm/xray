// Exercise the actual npm tarballs without executing package lifecycle scripts.
import { mkdtempSync, mkdirSync, cpSync, copyFileSync, writeFileSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { spawnSync } from 'node:child_process'

const repo = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const [target, executable] = process.argv.slice(2)
if (!target || !executable || !/^(win32|darwin|linux)-(x64|arm64)(-musl)?$/.test(target)) throw new Error('expected package target and executable')
const temp = mkdtempSync(join(tmpdir(), 'xray-package-smoke-'))
const npm = process.platform === 'win32' ? 'npm.cmd' : 'npm'
function run(command, args, cwd) {
  const windowsNpm = process.platform === 'win32' && command === npm
  const quoted = windowsNpm ? args.map(value => `"${value.replaceAll('"', '""')}"`) : args
  const result = spawnSync(command, quoted, { cwd, encoding: 'utf8', shell: windowsNpm, env: { ...process.env, npm_config_audit: 'false', npm_config_fund: 'false' } })
  if (result.status !== 0) throw new Error(`${command} failed (${result.status}): ${result.stdout}\n${result.stderr}`)
  return result.stdout
}
try {
  mkdirSync(join(temp, 'scripts'), { recursive: true })
  mkdirSync(join(temp, 'npm'), { recursive: true })
  cpSync(join(repo, 'npm/xray'), join(temp, 'npm/xray'), { recursive: true })
  for (const file of ['Cargo.toml', 'LICENSE', 'README.md', 'THIRD_PARTY_NOTICES']) copyFileSync(join(repo, file), join(temp, file))
  copyFileSync(join(repo, 'scripts/generate-packages.mjs'), join(temp, 'scripts/generate-packages.mjs'))
  copyFileSync(resolve(executable), join(temp, `xray.${target}${target.startsWith('win32-') ? '.exe' : ''}`))
  run(process.execPath, [join(temp, 'scripts/generate-packages.mjs'), '--manifests-only'], temp)
  const pack = dir => {
    const result = JSON.parse(run(npm, ['pack', '--json', '--ignore-scripts', '--offline', '--pack-destination', temp, dir], temp))
    return (Array.isArray(result) ? result[0] : Object.values(result)[0]).filename
  }
  const platformTar = pack(join(temp, `npm/xray.${target}`))
  const wrapperTar = pack(join(temp, 'npm/xray'))
  const consumer = join(temp, 'consumer')
  mkdirSync(consumer)
  writeFileSync(join(consumer, 'package.json'), '{"private":true}')
  run(npm, ['install', '--ignore-scripts', '--offline', '--no-audit', '--no-fund', join(temp, platformTar), join(temp, wrapperTar)], consumer)
  for (const name of ['xray', `xray.${target}`]) {
    const notice = readFileSync(join(consumer, 'node_modules/@pnpm', name, 'THIRD_PARTY_NOTICES'), 'utf8')
    if (notice !== readFileSync(join(repo, 'THIRD_PARTY_NOTICES'), 'utf8')) throw new Error(`missing or mismatched notices in ${name}`)
  }
  const launcher = join(consumer, 'node_modules/@pnpm/xray/bin/xray.mjs')
  const version = run(process.execPath, [launcher, '--version'], consumer)
  if (!version.includes('xray')) throw new Error('unexpected launcher version')
  const fixture = join(temp, 'fixture')
  mkdirSync(fixture)
  writeFileSync(join(fixture, 'package.json'), '{"name":"xray-release-smoke","version":"1.0.0","main":"index.js"}')
  writeFileSync(join(fixture, 'index.js'), '// @flow\nimport type { T } from "sample-types";\n')
  const scan = JSON.parse(run(process.execPath, [launcher, '--package', fixture, '--json'], consumer))
  if (!scan.complete || !scan.packages.some(pkg => pkg.findings.some(finding => finding.dependency === 'sample-types'))) throw new Error('installed launcher did not scan Flow fixture')
  console.log(`Installed ${target} tarballs offline and scanned Flow through the packaged launcher`)
} finally { rmSync(temp, { recursive: true, force: true }) }
