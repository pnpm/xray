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
