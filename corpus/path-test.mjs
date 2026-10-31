// Cross-platform smoke test for Node/Rust realpath handling and verified patches.
// XRAY_TYPESCRIPT must name an installed TypeScript 5.9.3 package directory.
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const repo = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const binary = process.env.XRAY_XRAY_BINARY || join(repo, 'target', 'debug', process.platform === 'win32' ? 'xray.exe' : 'xray');
const typescript = process.env.XRAY_TYPESCRIPT;
if (!typescript) throw new Error('Set XRAY_TYPESCRIPT to a locally installed TypeScript 5.9.3 package directory');
const manifest = JSON.parse(await readFile(join(typescript, 'package.json'), 'utf8'));
if (manifest.name !== 'typescript' || manifest.version !== '5.9.3') throw new Error('XRAY_TYPESCRIPT must point to TypeScript 5.9.3');

function run(command, args, cwd) {
  const result = spawnSync(command, args, { cwd, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 });
  if (result.error) throw result.error;
  return result;
}
function expect(result, status, context) {
  if (result.status !== status) throw new Error(`${context}: expected exit ${status}, got ${result.status}\n${result.stdout}\n${result.stderr}`);
}

const root = await mkdtemp(join(tmpdir(), 'xray-path-smoke-'));
try {
  const modules = join(root, 'node_modules');
  const pkg = join(modules, 'xray-path-fixture');
  await mkdir(pkg, { recursive: true });
  await cp(typescript, join(modules, 'typescript'), { recursive: true, dereference: true });
  await writeFile(join(root, 'package.json'), JSON.stringify({ private: true, type: 'module' }));
  await writeFile(join(root, 'pnpm-workspace.yaml'), 'packages: []\n');
  await writeFile(join(root, 'tsconfig.json'), JSON.stringify({ compilerOptions: { target: 'es2022', module: 'nodenext', moduleResolution: 'nodenext', types: [], noEmit: true }, include: ['index.mts'] }));
  await writeFile(join(root, 'index.mts'), "import { value } from 'xray-path-fixture';\nconst result: number = value;\nexport { result };\n");
  await writeFile(join(pkg, 'package.json'), JSON.stringify({ name: 'xray-path-fixture', version: '1.0.0', type: 'module', main: 'index.mjs', types: 'index.d.mts' }));
  const javascript = "/** @type {number} */\nexport const value = 123;\nthrow new Error('fixture JavaScript must not execute');\n";
  const original = 'export declare const;\nexport declare const value: number;\n';
  await writeFile(join(pkg, 'index.mjs'), javascript);
  await writeFile(join(pkg, 'index.d.mts'), original);
  const tsc = join(modules, 'typescript', 'lib', 'tsc.js');
  const before = run(process.execPath, [tsc, '--noEmit'], root);
  if (before.status === 0 || !`${before.stdout}${before.stderr}`.includes('TS1123')) throw new Error(`Expected malformed declaration before repair:\n${before.stdout}\n${before.stderr}`);
  const workspace = join(root, 'pnpm-workspace.yaml');
  const beforeWorkspace = await readFile(workspace, 'utf8');
  const patch = join(root, 'patches', 'xray-path-fixture@1.0.0.patch');
  const preview = run(binary, ['--fix', '--regenerate-types', '--dry-run', root], root);
  expect(preview, 2, 'declaration repair preview');
  if (!preview.stdout.includes('xray-path-fixture@1.0.0')) throw new Error(`Expected a verified declaration repair:\n${preview.stdout}\n${preview.stderr}`);
  if (await readFile(workspace, 'utf8') !== beforeWorkspace || await readFile(patch, 'utf8').then(() => true, () => false)) throw new Error('Dry run changed workspace settings or created a patch');
  const applied = run(binary, ['--fix', '--regenerate-types', root], root);
  expect(applied, 2, 'declaration repair apply');
  const patchText = await readFile(patch, 'utf8');
  if (!patchText.includes('index.d.mts') || !(await readFile(workspace, 'utf8')).includes('patchedDependencies')) throw new Error('Missing patch artifact or patchedDependencies setting');
  if (await readFile(join(pkg, 'index.d.mts'), 'utf8') !== original) throw new Error('Repair modified installed package directly');
  // Apply only to a test copy: the actual command must never modify the package.
  const copy = join(root, 'package-copy');
  await cp(pkg, copy, { recursive: true });
  const git = run('git', ['apply', patch], copy);
  expect(git, 0, 'applying patch to test copy');
  await writeFile(join(pkg, 'index.d.mts'), await readFile(join(copy, 'index.d.mts')));
  expect(run(process.execPath, [tsc, '--noEmit'], root), 0, 'type checking patched package');
  const scan = run(binary, ['--json', root], root);
  expect(scan, 0, 'scanning patched project');
  const output = JSON.parse(scan.stdout);
  if (!output.complete || output.diagnostics.length) throw new Error(`Patched project scan is incomplete: ${scan.stdout}`);
  console.log('PASS cross-platform declaration regeneration, patch application, and type checking');
} finally {
  await rm(root, { recursive: true, force: true });
}
