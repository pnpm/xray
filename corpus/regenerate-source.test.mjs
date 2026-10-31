// Adversarial behavior checks for declaration regeneration; no registry code.
// XRAY_TYPESCRIPT points to an installed TypeScript 5.9.3 package directory.
import { strict as assert } from 'node:assert'
import { spawnSync } from 'node:child_process'
import { mkdtemp, mkdir, readFile, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

const repo = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const bridge = join(repo, 'src/regenerate-bridge.cjs')
const typescript = process.env.XRAY_TYPESCRIPT
const unavailable = !typescript && 'set XRAY_TYPESCRIPT to an installed TypeScript 5.9.3 package'

async function fixture(declaration, consumer, options = {}) {
  const root = await mkdtemp(join(tmpdir(), 'xray-virtual-test-'))
  const pkg = join(root, 'node_modules/xray-test-fixture')
  await mkdir(pkg, { recursive: true })
  await symlink(resolve(typescript), join(root, 'node_modules/typescript'), 'junction')
  await writeFile(join(root, 'package.json'), JSON.stringify({ private: true, type: 'module' }))
  await writeFile(join(root, 'tsconfig.json'), JSON.stringify({ compilerOptions: { module: 'nodenext', moduleResolution: 'nodenext', target: 'esnext', strict: true, types: [], noEmit: true }, include: ['consumer.mts'], ...(options.references ? { references: [{ path: './other' }] } : {}) }))
  await writeFile(join(root, 'consumer.mts'), consumer)
  await writeFile(join(pkg, 'package.json'), JSON.stringify({ name: 'xray-test-fixture', version: '1.0.0', type: 'module', exports: { types: './index.d.mts', default: './index.mjs' } }))
  await writeFile(join(pkg, 'index.d.mts'), declaration)
  await writeFile(join(pkg, 'index.mjs'), "export const count = 1; throw new Error('package code must not execute');\n")
  if (options.sibling) await writeFile(join(pkg, 'index.mts'), 'export const count = 1;\n')
  if (options.relative) await writeFile(join(pkg, 'contract.d.ts'), 'export interface Contract { deep: { kind: "left" | "right" } }\n')
  const roots = join(root, 'roots.json')
  await writeFile(roots, JSON.stringify([pkg]))
  return { root, pkg, roots }
}

function invoke({ root, roots }) {
  const result = spawnSync(process.execPath, [bridge, root, roots], { cwd: root, encoding: 'utf8', timeout: 30_000, maxBuffer: 8 * 1024 * 1024 })
  assert.equal(result.status, 0, result.stderr)
  return JSON.parse(result.stdout)
}

test('removes only an unresolved final identifier while preserving the declaration', { skip: unavailable }, async () => {
  const source = 'export interface Contract { readonly item: "fixed" }\nexport declare const count: number;\n// Retain this comment.\nstray\n'
  const context = await fixture(source, 'import { count, type Contract } from "xray-test-fixture";\nconst a: Contract = { item: "fixed" }; void [a, count];\n')
  try {
    const result = invoke(context)
    assert.equal(result.files.length, 1, JSON.stringify(result.messages))
    assert.equal(result.files[0].replacement, source.replace('stray', ''))
    await writeFile(join(context.pkg, 'index.d.mts'), result.files[0].replacement)
    const tsc = spawnSync(process.execPath, [join(context.root, 'node_modules/typescript/bin/tsc'), '--noEmit'], { cwd: context.root, encoding: 'utf8' })
    assert.equal(tsc.status, 0, tsc.stdout + tsc.stderr)
  } finally { await rm(context.root, { recursive: true, force: true }) }
})

test('allows an undeclared runtime export when removing an orphan identifier', { skip: unavailable }, async () => {
  const source = 'export declare const count: number;\nstray\n'
  const context = await fixture(source, 'import { count } from "xray-test-fixture"; void count;\n')
  try {
    await writeFile(join(context.pkg, 'index.mjs'), 'export const count = 1;\nexport const internal = 2;\n')
    const result = invoke(context)
    assert.equal(result.files.length, 1, JSON.stringify(result.messages))
    assert.equal(result.files[0].replacement, source.replace('stray', ''))
  } finally { await rm(context.root, { recursive: true, force: true }) }
})

for (const [name, source] of [
  ['known identifier', 'export declare const count: number;\ncount;\n'],
  ['nonterminal identifier', 'stray;\nexport declare const count: number;\n'],
  ['call expression', 'export declare const count: number;\nstray();\n'],
  ['additional declaration error', 'export declare const;\nstray\n'],
  ['runtime export mismatch', 'export declare const count: number;\nexport declare const hidden: string;\nstray\n'],
]) {
  test(`does not repair ${name}`, { skip: unavailable }, async () => {
    const context = await fixture(source, 'import { count } from "xray-test-fixture"; void count;\n')
    try { assert.equal(invoke(context).files.length, 0) }
    finally { await rm(context.root, { recursive: true, force: true }) }
  })
}

const declaration = 'export interface Contract { readonly deep: { kind: "left" | "right" } }\nexport type Choice = Contract["deep"]["kind"];\nexport var count = 1;\n'
const consumer = 'import { count, type Contract, type Choice } from "xray-test-fixture";\nconst a: number = count;\nconst b: Choice = "left";\n// @ts-expect-error invalid exported literal\nconst c: Choice = "other";\nconst d: Contract = { deep: { kind: b } };\nvoid [a,c,d];\n'

test('virtual source retains exported types and rejects invalid consumer values', { skip: unavailable }, async () => {
  const context = await fixture(declaration, consumer)
  try {
    const result = invoke(context)
    assert.equal(result.files.length, 1, JSON.stringify(result.messages))
    assert.match(result.files[0].replacement, /interface Contract/)
    assert.match(result.files[0].replacement, /type Choice/)
    assert.equal(await readFile(join(context.pkg, 'index.d.mts'), 'utf8'), declaration)
    // Apply only inside this disposable synthetic fixture for independent tsc.
    await writeFile(join(context.pkg, 'index.d.mts'), result.files[0].replacement)
    const tsc = spawnSync(process.execPath, [join(context.root, 'node_modules/typescript/bin/tsc'), '--noEmit'], { cwd: context.root, encoding: 'utf8' })
    assert.equal(tsc.status, 0, tsc.stdout + tsc.stderr)
  } finally { await rm(context.root, { recursive: true, force: true }) }
})

test('rejects replacement that drops an unused declaration-only interface', { skip: unavailable }, async () => {
  const context = await fixture('export interface HiddenContract { secret: string }\nexport declare const;\n', 'import { count } from "xray-test-fixture"; void count;\n')
  try {
    const result = invoke(context)
    assert.equal(result.files.length, 0)
    assert.match(result.messages.join('\n'), /HiddenContract/)
  } finally { await rm(context.root, { recursive: true, force: true }) }
})

test('does not offer a repair if another consumer error remains', { skip: unavailable }, async () => {
  const context = await fixture(declaration, `${consumer}\nconst unrelated: number = "wrong"; void unrelated;\n`)
  try {
    const result = invoke(context)
    assert.equal(result.files.length, 0)
    assert.match(result.messages.join('\n'), /TS2322/)
  } finally { await rm(context.root, { recursive: true, force: true }) }
})

test('rejects virtual source when a real adjacent implementation exists', { skip: unavailable }, async () => {
  const context = await fixture(declaration, consumer, { sibling: true })
  try {
    const result = invoke(context)
    assert.equal(result.files.length, 0)
    assert.match(result.messages.join('\n'), /No eligible/)
  } finally { await rm(context.root, { recursive: true, force: true }) }
})

test('refuses a missing project reference', { skip: unavailable }, async () => {
  const context = await fixture(declaration, consumer, { references: true })
  try {
    const result = invoke(context)
    assert.equal(result.files.length, 0)
    assert.match(result.messages.join('\n'), /project reference is missing/i)
  } finally { await rm(context.root, { recursive: true, force: true }) }
})

test('preserves a relative type re-export through virtual source', { skip: unavailable }, async () => {
  const source = 'export type { Contract } from "./contract.js";\nexport var count = 1;\n'
  const use = 'import { count, type Contract } from "xray-test-fixture";\nconst a: Contract = { deep: { kind: "left" } }; void [a, count];\n'
  const context = await fixture(source, use, { relative: true })
  try {
    const result = invoke(context)
    assert.equal(result.files.length, 1, JSON.stringify(result.messages))
    assert.match(result.files[0].replacement, /contract\.js/)
  } finally { await rm(context.root, { recursive: true, force: true }) }
})

test('does not propose a declaration without adjacent shipped JavaScript', { skip: unavailable }, async () => {
  const context = await fixture(declaration, consumer)
  try {
    await rm(join(context.pkg, 'index.mjs'))
    const result = invoke(context)
    assert.equal(result.files.length, 0)
    assert.match(result.messages.join('\n'), /No eligible/)
  } finally { await rm(context.root, { recursive: true, force: true }) }
})

test('rejects virtual class with an uninitialized field under strict checking', { skip: unavailable }, async () => {
  const source = 'export class Holder { readonly item: { inner: "fixed" } }\nexport var count = 1;\n'
  const use = 'import { count, Holder } from "xray-test-fixture";\ndeclare const h: Holder;\nconst good: "fixed" = h.item.inner;\nvoid [good, count];\n'
  const context = await fixture(source, use)
  try {
    // Match the runtime value-export surface without executing package code.
    await writeFile(join(context.pkg, 'index.mjs'), 'export class Holder {}\nexport const count = 1;\nthrow new Error("must not execute");\n')
    const result = invoke(context)
    assert.equal(result.files.length, 0)
    assert.match(result.messages.join('\n'), /TS2564/)
  } finally { await rm(context.root, { recursive: true, force: true }) }
})

test('preserves an exported abstract class and nested literal property', { skip: unavailable }, async () => {
  const source = 'export abstract class Holder { abstract readonly item: { inner: "fixed" } }\nexport var count = 1;\n'
  const use = 'import { count, Holder } from "xray-test-fixture";\ndeclare const h: Holder;\nconst good: "fixed" = h.item.inner;\n// @ts-expect-error original literal must remain narrow\nconst bad: "other" = h.item.inner;\nvoid [good, bad, count];\n'
  const context = await fixture(source, use)
  try {
    await writeFile(join(context.pkg, 'index.mjs'), 'export class Holder {}\nexport const count = 1;\nthrow new Error("must not execute");\n')
    const result = invoke(context)
    assert.equal(result.files.length, 1, JSON.stringify(result.messages))
    assert.match(result.files[0].replacement, /abstract class Holder/)
    await writeFile(join(context.pkg, 'index.d.mts'), result.files[0].replacement)
    const tsc = spawnSync(process.execPath, [join(context.root, 'node_modules/typescript/bin/tsc'), '--noEmit'], { cwd: context.root, encoding: 'utf8' })
    assert.equal(tsc.status, 0, tsc.stdout + tsc.stderr)
  } finally { await rm(context.root, { recursive: true, force: true }) }
})

async function referencedFixture() {
  const context = await fixture('export declare const count: number;\n', 'void 0;\n')
  const common = { composite: true, module: 'nodenext', moduleResolution: 'nodenext', target: 'esnext', strict: true, types: [], skipLibCheck: false }
  for (const name of ['a', 'b']) {
    const dir = join(context.root, name)
    await mkdir(dir)
    await writeFile(join(dir, 'tsconfig.json'), JSON.stringify({ compilerOptions: { ...common, outDir: './out' }, include: ['index.mts'] }))
    await writeFile(join(dir, 'index.mts'), name === 'a' ? 'import { count } from "xray-test-fixture"; export const value: number = count;\n' : 'export const other: number = 1;\n')
  }
  await writeFile(join(context.root, 'tsconfig.json'), JSON.stringify({ files: [], references: [{ path: './a' }, { path: './b' }] }))
  const build = spawnSync(process.execPath, [join(context.root, 'node_modules/typescript/bin/tsc'), '--build'], { cwd: context.root, encoding: 'utf8' })
  assert.equal(build.status, 0, build.stdout + build.stderr)
  await writeFile(join(context.pkg, 'index.d.mts'), 'export const count: number = 1;\nexport type Choice = "left" | "right";\n')
  return context
}

test('verifies all built TypeScript project references independently', { skip: unavailable }, async () => {
  const context = await referencedFixture()
  try {
    const result = invoke(context)
    assert.equal(result.files.length, 1, JSON.stringify(result.messages))
    assert.match(result.files[0].replacement, /"left" \| "right"/)
    await rm(join(context.root, 'b/out/index.d.mts'))
    const unbuilt = invoke(context)
    assert.equal(unbuilt.files.length, 0)
    assert.match(unbuilt.messages.join('\n'), /unbuilt declaration outputs/i)
    await writeFile(join(context.root, 'b/out/index.d.mts'), 'export declare const other: number;\n')
    await writeFile(join(context.root, 'b/index.mts'), 'export const other: string = "fresh";\n')
    const stale = invoke(context)
    assert.equal(stale.files.length, 0)
    assert.match(stale.messages.join('\n'), /stale or unverified/)
    await writeFile(join(context.root, 'b/index.mts'), 'export const other: number = "bad";\n')
    const failed = invoke(context)
    assert.equal(failed.files.length, 0)
    assert.match(failed.messages.join('\n'), /TS2322/)
  } finally { await rm(context.root, { recursive: true, force: true }) }
})

test('rejects cyclic TypeScript project references', { skip: unavailable }, async () => {
  const context = await referencedFixture()
  try {
    await writeFile(join(context.root, 'a/tsconfig.json'), JSON.stringify({ compilerOptions: { composite: true }, files: ['index.mts'], references: [{ path: '../b' }] }))
    await writeFile(join(context.root, 'b/tsconfig.json'), JSON.stringify({ compilerOptions: { composite: true }, files: ['index.mts'], references: [{ path: '../a' }] }))
    const result = invoke(context)
    assert.equal(result.files.length, 0)
    assert.match(result.messages.join('\n'), /cyclic/i)
  } finally { await rm(context.root, { recursive: true, force: true }) }
})

test('downstream references see declarations inferred after repair', { skip: unavailable }, async () => {
  const context = await fixture('export declare const count: any;\n', 'void 0;\n')
  try {
    const common = { composite: true, module: 'nodenext', moduleResolution: 'nodenext', target: 'esnext', strict: true, types: [], skipLibCheck: false, outDir: './out' }
    for (const name of ['a', 'b']) await mkdir(join(context.root, name))
    await writeFile(join(context.root, 'a/tsconfig.json'), JSON.stringify({ compilerOptions: common, include: ['index.mts'] }))
    await writeFile(join(context.root, 'a/index.mts'), 'import { count } from "xray-test-fixture"; export const value = count;\n')
    await writeFile(join(context.root, 'b/tsconfig.json'), JSON.stringify({ compilerOptions: common, include: ['index.mts'], references: [{ path: '../a' }] }))
    await writeFile(join(context.root, 'tsconfig.json'), JSON.stringify({ files: [], references: [{ path: './b' }] }))
    await writeFile(join(context.root, 'b/index.mts'), 'import { value } from "../a/out/index.mjs"; export const text: string = value;\n')
    const build = spawnSync(process.execPath, [join(context.root, 'node_modules/typescript/bin/tsc'), '--build'], { cwd: context.root, encoding: 'utf8' })
    assert.equal(build.status, 0, build.stdout + build.stderr)
    await writeFile(join(context.pkg, 'index.d.mts'), 'export const count = ;\n')
    await writeFile(join(context.pkg, 'index.mjs'), '/** @type {number} */ export const count = 1;\n')
    const result = invoke(context)
    assert.equal(result.files.length, 0, JSON.stringify(result))
    assert.match(result.messages.join('\n'), /TS2322/)
  } finally { await rm(context.root, { recursive: true, force: true }) }
})
