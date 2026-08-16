import { spawnSync } from 'node:child_process'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import process from 'node:process'
import test from 'node:test'
import { fileURLToPath } from 'node:url'
import { platformTarget } from '../platform.mjs'

test('names the package the installer would have chosen', () => {
  assert.equal(platformTarget({ platform: 'darwin', arch: 'arm64' }), 'darwin-arm64')
  assert.equal(platformTarget({ platform: 'win32', arch: 'x64' }), 'win32-x64')
  assert.equal(platformTarget({ platform: 'linux', arch: 'x64', musl: false }), 'linux-x64')
})

test('separates musl Linux from glibc Linux', () => {
  assert.equal(platformTarget({ platform: 'linux', arch: 'x64', musl: true }), 'linux-x64-musl')
  assert.equal(platformTarget({ platform: 'linux', arch: 'arm64', musl: true }), 'linux-arm64-musl')
})

test('only Linux is split by libc', () => {
  assert.equal(platformTarget({ platform: 'darwin', arch: 'arm64', musl: true }), 'darwin-arm64')
})

test('every target it can name has a package published for it', async () => {
  const manifest = JSON.parse(
    await readFile(new URL('../package.json', import.meta.url), 'utf8')
  )
  const published = Object.keys(manifest.optionalDependencies)
  for (const platform of ['darwin', 'linux', 'win32']) {
    for (const arch of ['x64', 'arm64']) {
      for (const musl of [true, false]) {
        const target = platformTarget({ platform, arch, musl })
        assert.ok(
          published.includes(`@pnpm/xray.${target}`),
          `${target} can be selected at runtime but is not published`
        )
      }
    }
  }
})

test('says what to do when no binary is installed for this machine', () => {
  const launcher = fileURLToPath(new URL('../bin/xray.mjs', import.meta.url))
  const { status, stderr } = spawnSync(process.execPath, [launcher, '--help'], {
    encoding: 'utf8',
  })
  assert.equal(status, 1)
  assert.match(stderr, /no prebuilt binary for/)
  assert.match(stderr, /@pnpm\/xray\./)
})
