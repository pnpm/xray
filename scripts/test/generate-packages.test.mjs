import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

const repo = resolve(dirname(fileURLToPath(import.meta.url)), '../..')

test('binary packages and wrapper retain the bundled parser notice', async () => {
  const root = await mkdtemp(join(tmpdir(), 'xray-packaging-'))
  try {
    for (const file of ['Cargo.toml', 'LICENSE', 'THIRD_PARTY_NOTICES', 'README.md', 'npm/xray/package.json', 'scripts/generate-packages.mjs']) {
      const destination = join(root, file)
      await mkdir(dirname(destination), { recursive: true })
      await copyFile(join(repo, file), destination)
    }
    const wrapper = JSON.parse(await readFile(join(root, 'npm/xray/package.json'), 'utf8'))
    for (const name of Object.keys(wrapper.optionalDependencies)) {
      const target = name.slice('@pnpm/xray.'.length)
      await writeFile(join(root, `xray.${target}${target.startsWith('win32-') ? '.exe' : ''}`), 'binary fixture')
    }
    const result = spawnSync(process.execPath, [join(root, 'scripts/generate-packages.mjs')], { encoding: 'utf8' })
    assert.equal(result.status, 0, result.stderr)
    const notice = await readFile(join(root, 'THIRD_PARTY_NOTICES'), 'utf8')
    const parserLicense = await readFile(join(repo, 'vendor/flow-parser/LICENSE'), 'utf8')
    assert.ok(notice.includes(parserLicense))
    assert.ok(notice.includes('Copyright (c) 2024-present VoidZero Inc. & Contributors'))
    assert.ok(notice.includes('Copyright (c) Microsoft Corporation.'))
    assert.ok(notice.includes('- oxc_span@'))
    assert.ok(notice.includes('- windows-sys@'))
    for (const name of Object.keys(wrapper.optionalDependencies)) {
      const target = name.slice('@pnpm/xray.'.length)
      const directory = join(root, `npm/xray.${target}`)
      const manifest = JSON.parse(await readFile(join(directory, 'package.json'), 'utf8'))
      assert.ok(manifest.files.includes('THIRD_PARTY_NOTICES'))
      assert.equal(await readFile(join(directory, 'THIRD_PARTY_NOTICES'), 'utf8'), notice)
      assert.equal(await readFile(join(directory, target.startsWith('win32-') ? 'xray.exe' : 'xray'), 'utf8'), 'binary fixture')
    }
    assert.ok(wrapper.files.includes('THIRD_PARTY_NOTICES'))
    assert.equal(await readFile(join(root, 'npm/xray/THIRD_PARTY_NOTICES'), 'utf8'), notice)
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})
