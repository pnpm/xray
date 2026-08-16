#!/usr/bin/env node
// The binary itself travels in a `@pnpm/xray.<target>` package, one per
// platform, listed in `optionalDependencies`. The package manager installs only
// the one whose `os`/`cpu`/`libc` match the host, so this wrapper's job is to
// name that target the same way `scripts/generate-packages.mjs` did and hand
// over to the executable inside it.
import { spawnSync } from 'node:child_process'
import { createRequire } from 'node:module'
import os from 'node:os'
import process from 'node:process'

const EXECUTABLE = process.platform === 'win32' ? 'xray.exe' : 'xray'

run(resolveExecutable())

function run (executable) {
  const { status, error, signal } = spawnSync(executable, process.argv.slice(2), {
    stdio: 'inherit',
  })
  if (error) {
    console.error(`xray: could not run ${executable}: ${error.message}`)
    process.exit(1)
  }
  // A child killed by a signal reports no exit status; 128 + signal number is
  // the convention shells use for it.
  if (signal) process.exit(128 + (os.constants.signals[signal] ?? 0))
  process.exit(status ?? 1)
}

function resolveExecutable () {
  const target = platformTarget()
  const require = createRequire(import.meta.url)
  try {
    return require.resolve(`@pnpm/xray.${target}/${EXECUTABLE}`)
  } catch {
    console.error(
      `xray: no prebuilt binary for ${target}.\n` +
        `Either this platform is not supported yet, or the optional dependency\n` +
        `@pnpm/xray.${target} was skipped during install. Reinstalling without\n` +
        `--no-optional usually fixes the second case.`
    )
    process.exit(1)
  }
}

function platformTarget () {
  const target = `${process.platform}-${process.arch}`
  return process.platform === 'linux' && isMusl() ? `${target}-musl` : target
}

// musl builds report no glibc version, which is the only signal Node exposes
// without shelling out to `ldd`.
function isMusl () {
  const report = typeof process.report?.getReport === 'function' ? process.report.getReport() : null
  return report != null && !report.header?.glibcVersionRuntime
}
