// Generates the per-platform `@pnpm/xray.<target>` packages, each carrying one
// prebuilt executable, and patches the `@pnpm/xray` wrapper's
// `optionalDependencies` to reference them at the wrapper's own version.
//
// CI cross-compiles one executable per Rust target and collects them at the
// repo root as `xray.<packageTarget>` (`.exe` on Windows), then runs this.
//
// `packageTarget` is what the wrapper's `platformTarget()` computes at runtime,
// so the two must stay in step: `${process.platform}-${process.arch}`, plus a
// `-musl` suffix on musl Linux.
import * as fs from 'node:fs'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const TARGETS = [
  { packageTarget: 'win32-x64', rustTarget: 'x86_64-pc-windows-msvc', os: 'win32', cpu: 'x64' },
  { packageTarget: 'win32-arm64', rustTarget: 'aarch64-pc-windows-msvc', os: 'win32', cpu: 'arm64' },
  { packageTarget: 'darwin-x64', rustTarget: 'x86_64-apple-darwin', os: 'darwin', cpu: 'x64' },
  { packageTarget: 'darwin-arm64', rustTarget: 'aarch64-apple-darwin', os: 'darwin', cpu: 'arm64' },
  {
    packageTarget: 'linux-x64',
    rustTarget: 'x86_64-unknown-linux-gnu',
    os: 'linux',
    cpu: 'x64',
    libc: 'glibc',
  },
  {
    packageTarget: 'linux-arm64',
    rustTarget: 'aarch64-unknown-linux-gnu',
    os: 'linux',
    cpu: 'arm64',
    libc: 'glibc',
  },
  {
    packageTarget: 'linux-x64-musl',
    rustTarget: 'x86_64-unknown-linux-musl',
    os: 'linux',
    cpu: 'x64',
    libc: 'musl',
  },
  {
    packageTarget: 'linux-arm64-musl',
    rustTarget: 'aarch64-unknown-linux-musl',
    os: 'linux',
    cpu: 'arm64',
    libc: 'musl',
  },
]

const WRAPPER_DIR = resolve(fileURLToPath(import.meta.url), '../../npm/xray')
const PACKAGES_DIR = resolve(WRAPPER_DIR, '..')
const REPO_ROOT = resolve(PACKAGES_DIR, '..')

const wrapperManifestPath = resolve(WRAPPER_DIR, 'package.json')
const wrapperManifest = JSON.parse(fs.readFileSync(wrapperManifestPath, 'utf8'))
const { version } = wrapperManifest

const crateVersion = /^version = "(.+)"$/m.exec(
  fs.readFileSync(resolve(REPO_ROOT, 'Cargo.toml'), 'utf8')
)?.[1]
if (crateVersion !== version) {
  throw new Error(
    `Cargo.toml is at ${crateVersion} but npm/xray/package.json is at ${version} — ` +
      'the published binary would not match the version it is published under'
  )
}

// Without an artifact the package would publish an empty executable, so a
// missing one has to stop the release rather than be skipped.
const requireArtifacts = !process.argv.includes('--manifests-only')

const optionalDependencies = {}
for (const target of TARGETS) {
  const executable = target.os === 'win32' ? 'xray.exe' : 'xray'
  const packageName = `@pnpm/xray.${target.packageTarget}`
  const packageDir = resolve(PACKAGES_DIR, `xray.${target.packageTarget}`)
  const artifact = resolve(REPO_ROOT, `xray.${target.packageTarget}${target.os === 'win32' ? '.exe' : ''}`)

  fs.mkdirSync(packageDir, { recursive: true })
  fs.writeFileSync(
    resolve(packageDir, 'package.json'),
    `${JSON.stringify(
      {
        name: packageName,
        version,
        description: `The ${target.packageTarget} binary for @pnpm/xray`,
        license: wrapperManifest.license,
        homepage: wrapperManifest.homepage,
        bugs: wrapperManifest.bugs,
        repository: {
          ...wrapperManifest.repository,
          directory: `npm/xray.${target.packageTarget}`,
        },
        os: [target.os],
        cpu: [target.cpu],
        ...(target.libc ? { libc: [target.libc] } : {}),
        files: [executable],
      },
      null,
      2
    )}\n`
  )
  fs.copyFileSync(resolve(REPO_ROOT, 'LICENSE'), resolve(packageDir, 'LICENSE'))

  if (fs.existsSync(artifact)) {
    fs.copyFileSync(artifact, resolve(packageDir, executable))
    fs.chmodSync(resolve(packageDir, executable), 0o755)
  } else if (requireArtifacts) {
    throw new Error(`missing build artifact ${artifact} for ${packageName}`)
  }

  optionalDependencies[packageName] = version
}

wrapperManifest.optionalDependencies = optionalDependencies
fs.writeFileSync(wrapperManifestPath, `${JSON.stringify(wrapperManifest, null, 2)}\n`)

// The wrapper publishes from its own directory, so the files npm shows on the
// package page have to be next to it rather than at the repo root.
for (const file of ['README.md', 'LICENSE']) {
  fs.copyFileSync(resolve(REPO_ROOT, file), resolve(WRAPPER_DIR, file))
}

console.log(`generated ${TARGETS.length} platform packages at version ${version}`)
