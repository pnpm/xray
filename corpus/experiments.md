# Workspace experiments

These are observations from public workspace checkouts, not an exhaustive survey
of npm. Both installs used Linux x64, Node 24.21.0, and disabled lifecycle
scripts. The project-selected pnpm versions are given below. Tests invoke the
Vitest JavaScript entry point directly, so they do not exercise pnpm's runtime
resolution hook or `NODE_PATH` fallback.

## Vue core: an optional template engine in the global store

At `vuejs/core` commit `24c9d0bffb2299635b8112a978d74a38723aaa10`, with
pnpm 12.11.2:

```sh
pnpm install --frozen-lockfile --ignore-scripts --config.hoist=false
node node_modules/vitest/vitest.mjs run --project 'unit*'
```

The isolated local-store install passed 183 test files and 3,932 tests (one
file and six tests skipped). Switching to the global store:

```sh
pnpm install --frozen-lockfile --ignore-scripts --config.hoist=false --config.enable-global-virtual-store=true
node node_modules/vitest/vitest.mjs run --project 'unit*'
```

The same test invocation failed two template compiler tests and reported three
unhandled `MODULE_NOT_FOUND: pug` errors from `@vue/consolidate@1.0.0`.
`pug@3.0.4` was a direct workspace development dependency, but the global-store
copy of the consolidator could not resolve it. The consolidator's manifest lists
`pug` only as a development dependency (`^3.0.0`); xray reports it as a runtime
reference. Adding the following entry to the checkout's `pnpm-workspace.yaml`,
then reinstalling with `--no-frozen-lockfile` and the global-store options above,
restored the passing result:

```yaml
packageExtensions:
  '@vue/consolidate@1.0.0':
    dependencies:
      pug: ^3.0.0
```

The unedited xray-generated extension for this package, which contains 53
optional peers including `pug`, was also installed and the same suite passed.
The single dependency entry above was tested separately. The consolidator supports
many optional engines, so promoting all references to required dependencies
would substantially change the dependency graph.
The two test runs also differ in their install history: pnpm exec started a
reinstall between them, which was interrupted while running lifecycle scripts;
the global-store test used direct Node invocation after that interruption.

## Vite: static scan only

At `vitejs/vite` commit `90bc034a97e7eccd0f3cbc69904fffa625a4c34d`, pnpm
12.15.0 installed 294 workspace projects with scripts disabled and hoisting
disabled, both with the local and global virtual stores. A root xray scan
completed in both cases and reported identical distinct package/version and
dependency pairs; its global-store output had duplicate records. Both scans
reported a parser warning in `@jridgewell/remapping@2.3.5` declarations.
No Vite functional suite completed, so these static findings are not confirmed
runtime failures.

## SvelteKit: focused tests pass in both layouts

At `sveltejs/kit` commit `3da859a9f161d7e57d74a109149b83076664da7d`,
pnpm 12.14.0 installed the 75-project workspace with scripts disabled and
hoisting disabled. From `packages/kit`, the project's focused unit test command
using the root Vitest executable was:

```sh
node ../../node_modules/vitest/vitest.mjs run -c vitest.kit.config.js
```

It passed 76 files and 1,124 tests (one skipped) both before and after switching
to the global virtual store with `--config.enable-global-virtual-store=true`.
The global-store `node_modules/vitest` symlink resolved into the store's `links`
directory. `node node_modules/@typescript/native/bin/tsc --noEmit` also passed
from `packages/kit` under the global store. These focused tests revealed no
failure; they do not establish that all workspace packages or integrations work.
A root Vitest run also included build-fixture tests whose generated artifacts
were absent, so its failures are not evidence of broken registry packages.

## Astro: focused tests pass in both layouts

At `withastro/astro` commit `10dae2f31486cc0a6a04d1f3d7f5ab7fa8889b30`,
pnpm 11.28.4 installed 577 workspace projects with scripts disabled and
hoisting disabled. The checkout needed generated workspace build artifacts before
its tests could run. From the repository root, the preparation was:

```sh
(cd packages/astro && node_modules/.bin/astro-scripts prebuild --to-string 'src/runtime/server/astro-island.ts' 'src/runtime/client/{idle,load,media,only,visible}.ts' && node_modules/.bin/astro-scripts build 'src/**/*.{ts,js}' --copy-wasm)
node_modules/.bin/turbo run build:ci --filter='astro^...'
(cd packages/integrations/preact && node_modules/.bin/astro-scripts build 'src/**/*.ts')
```

Turbo's nested pnpm commands warned that their automatic verification install
could not remove the modules directory without a terminal; the build tasks still
reported success. From `packages/astro`, the focused test command was:

```sh
node_modules/.bin/astro-scripts test 'test/units/**/*.test.ts' --strip-types --teardown ./test/units/teardown.ts
```

It passed 3,773 tests with one skipped. After reinstalling with
`--config.enable-global-virtual-store=true` (and
`--config.confirm-modules-purge=false` for this disposable checkout), the same
test command passed with the same counts. The root Vitest symlink resolved into
the global store's `links` directory. These tests did not expose a further
registry-package failure; integration and browser suites were not run.

## Follow-up: two type failures found in Vite's dependencies

The Vite scan identified `@volar/typescript@2.4.28`, whose reachable
`lib/protocol/createSys.d.ts` references `@volar/language-service` even though
the latter is only a development dependency of the published package. In a new
global-store workspace with `hoist: false`, install `@volar/typescript@2.4.28`
and `typescript@5.9.3`. An `index.ts` containing
`import * as volar from '@volar/typescript'; console.log(volar);` fails with
TS2307 when checked with:

```sh
node node_modules/typescript/bin/tsc --noEmit --module nodenext --moduleResolution nodenext --target es2022 index.ts
```

Applying the entire xray-generated extension and reinstalling makes that command
pass. The extension adds `@volar/language-service@2.4.28` as a dependency and
an optional `typescript` peer.

The same scan found an unresolved repair gap in
`@rollup/plugin-dynamic-import-vars@2.1.4`: its types import `estree` without a
declaration. A global-store workspace with this plugin, `rollup@4.64.9`,
`@types/estree@1.0.8`, and `typescript@5.9.3` fails TS2307 when compiling an
`index.ts` that imports the plugin, even when the consumer has the types package.
Use the command above with `--target esnext`. The complete generated extension
suggests optional peer `estree` and leaves this error unchanged. Replacing that
entry with dependency `@types/estree: 1.0.8` makes the check pass. This is a
known limitation: the source module specifier alone does not prove which
published `@types` package and version should be installed.

## Whole-workspace repair dry runs

On the four checkouts above, the full-project `xray --fix --dry-run` completed
for Vite, Vue, and SvelteKit; it left 88, 71, and 27 findings, respectively,
without selected repairs. On Astro, it stopped before writing because
`katex@0.16.28` ships Flow syntax in `katex.js`; that file is included by the
package's wildcard exports. This is an incomplete scan, not a clean result.

On the Vue global-store checkout, applying the entire generated proposal worked,
but `pnpm install --no-frozen-lockfile --ignore-scripts --config.hoist=false
--config.enable-global-virtual-store=true` failed under the configured trust
policy. A newly selected `@types/node@20.19.43` dependency tree required
`undici-types@6.21.0`, which pnpm rejected as a possible trust downgrade. The
proposal included several historical `@types/node` ranges from publishers'
development dependencies. The focused unit suite could not be rerun after that
full proposal without changing the policy or proposal. The earlier targeted
`@vue/consolidate` extension test is separate and did pass.

A subsequent planner revision preferred installed providers as optional peers,
which avoided introducing the rejected historical `@types/node` trees. The full
Vue proposal still could not be installed: a new `@vue/repl@4.7.2` declaration
repair selected `@vue/language-service@3.0.8`, whose dependency tree includes a
git dependency that pnpm rejected under `blockExoticSubdeps`. This confirms that
an author's development range alone does not establish that adding its whole
dependency tree is safe or permitted. No trust or exotic-dependency policy was
disabled for these tests.

A targeted trace repair was then tested from the original Vue workspace settings
and global-store install. Running `xray . --trace --fix -- node
node_modules/vitest/vitest.mjs run --project 'unit*'` preserved the failing
suite's exit status and generated optional peers for `pug` in
`@vue/consolidate@1.0.0` and `ms` in `vite@8.3.4`. The other observed missing
names, `then-pug` and `bufferutil`, had no selected installed provider. The
trace does not distinguish a caught lookup from a required one, so the `ms`
entry was not independently shown necessary. Reinstalling with scripts disabled
and the workspace's existing trust and exotic-dependency policies succeeded;
the unit suite then passed 183 files and 3,932 tests, with one file and six
tests skipped. Repeating the traced repair passed the suite and made no YAML
changes; it still observed the caught `bufferutil` lookup without selecting a
repair.

After Flow support was added, Astro's full dry run passed the prior KaTeX
parser blocker but stopped on `unenv@2.0.0-rc.24` declarations. Its published
`dist/runtime/node/process.d.mts` and `constants.d.mts` contain empty
`export declare const;` statements. A separate global-store consumer importing
`unenv/node/process` and `unenv/node/constants`, checked with TypeScript 5.9.3
in NodeNext mode, reports five TS1123 errors. The npm tarball is
`https://registry.npmjs.org/unenv/-/unenv-2.0.0-rc.24.tgz`, SHA-512 integrity
`sha512-i7qRCmY42zmCwnYlh9H2SvLEypEFGye5iRmEMKjcGi7zk9UquigRjFtTLz0TYqr0ZGLZhaMHl/foy1bZR+Cwlw==`.

The registry's `rc` tag was `2.0.0-rc.24` during this experiment. Releases
rc.5, rc.8, rc.10, rc.16, rc.20, rc.22 and rc.23 also contain the malformed
process declaration. The older rc.3 has syntactically valid declarations but
still failed the same type-checking probe due to Node type errors, so no version
override is verified. The JavaScript files export many named values; deleting
empty declaration statements would omit those API types and is not a verified
repair. An upstream declaration rebuild or a reviewed package patch is needed.
A broad Astro scan also encountered intentionally invalid test fixtures and an
unfinished editor-history file; those warnings alone do not establish package
runtime failures.
