import process from 'node:process'

// The name must match the `packageTarget` values in
// scripts/generate-packages.mjs: it is how the launcher finds the package the
// installer chose for this machine.
export function platformTarget ({
  platform = process.platform,
  arch = process.arch,
  musl = isMusl(),
} = {}) {
  const target = `${platform}-${arch}`
  return platform === 'linux' && musl ? `${target}-musl` : target
}

// musl builds report no glibc version, which is the only signal Node exposes
// without shelling out to `ldd`.
export function isMusl () {
  const report = typeof process.report?.getReport === 'function' ? process.report.getReport() : null
  return report != null && !report.header?.glibcVersionRuntime
}
