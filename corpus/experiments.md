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
repair. At that point no safe package patch had been verified; the follow-up below tests a candidate.
A broad Astro scan also encountered intentionally invalid test fixtures and an
unfinished editor-history file; those warnings alone do not establish package
runtime failures.

## Sass package imports

A separate pnpm global-store probe used `@material/button@14.0.0`, Vite 7.2.2,
Sass 1.94.0, and `@material/base@14.0.0` and
`@material/animation@14.0.0` as consumer dependencies, with hoisting disabled.
The published `mdc-button.import.scss` forwards both packages but the button
manifest declares neither. Building a Vite entry that imports a stylesheet with
`@use "@material/button/mdc-button.import.scss";` failed to find
`@material/base/mixins`. Adding only `@material/base` as a button dependency
made the build fail next on `@material/animation/variables`. Adding both as
version-scoped package-extension dependencies at `14.0.0` made the build
succeed. The tarball is
`https://registry.npmjs.org/@material/button/-/button-14.0.0.tgz`, integrity
`sha512-dqqHaJq0peyXBZupFzCjmvScrfljyVU66ZCS3oldsaaj5iz8sn33I/45Z4zPzdR5F5z8ExToHkRcXhakj1UEAA==`.
This specific probe is outside the four monorepo test suites above; it tests a
published package, not a workspace source file.

## Nuxt workspace

At `nuxt/nuxt` commit `4036aa703ebc4001458082e50df749d45a199e57`, pnpm
12.15.0 installed 68 projects with lifecycle scripts and hoisting disabled.
Running the unit and renderer Vitest projects directly under Node in the local
store yielded 224 passed files and three failures in one file; those assertions
expected a different spelling of the package-manager install command. A global
store reinstall succeeded and the Vitest symlink pointed into the global store.
The same broad command then reported 32 failed tests, including many temporary
project errors saying Nuxt could not be found. Re-running the representative
`packages/kit/test/app-config.test.ts` alone passed all three tests under the
global store. The broad run's additional failures were not traced to a published
package; test interaction and the direct Node invocation remain possible factors.
The attempt to exclude the known failing spec did not exclude it in this
workspace's Vitest project configuration.

A full-project `--fix --dry-run` reported an incomplete scan for the known
malformed `unenv@2.0.0-rc.24` declarations and for a vendored jasmine stylesheet
under `lz-string@1.5.0/tests`. The latter uses the legacy nonstandard IE
`*zoom` CSS hack; it is a test fixture, not evidence of a broken application
stylesheet. These two packages were excluded from the partial repair proposal.

A later experiment regenerated the two broken `unenv@2.0.0-rc.24`
declarations from the JavaScript shipped in the pinned tarball. TypeScript
5.9.3 declaration emit with the publisher's `@types/node@22.14.1` generated
files that passed a consumer check using typed default and named imports. A
TypeScript checker comparison with runtime module namespaces found the same
export names in each: 108 for process and 237 for constants. This is a reviewed
patch candidate, not a general automatic fix: other Node type versions can
still reveal unrelated incompatibilities in the package's internal declarations
(`@types/node@24.19.2` failed its `Process` interface check). Runtime export
comparison required a terminal because importing the process shim without one
failed to initialize its TTY.

After restricting repair scanning away from unrelated test fixtures, the Nuxt
whole-workspace dry run no longer reported the vendored jasmine CSS hack. It
still exited with an incomplete-scan diagnostic for the malformed unenv
constants declaration and excluded that package from the partial proposal.

## Browser-field replacement

The published `coinstring@2.0.0` imports Node's `crypto` and maps it to
`crypto-browserify` in its browser field, but lists the replacement only as a
development dependency. In a global-store, hoisting-disabled consumer with
`crypto-browserify@2.1.8` installed, esbuild 0.25.12 could not resolve the
browser mapping. Xray reported the replacement with evidence at
`lib/coinstring.js:1`. Applying its generated optional-peer extension and
reinstalling made the same browser-platform bundle command succeed. The command
marked `assert`, `buffer`, and `util` external to isolate the mapping; it did
not verify browser execution or provide those unrelated builtins. The exact
registry tarball is `https://registry.npmjs.org/coinstring/-/coinstring-2.0.0.tgz`,
with integrity `sha512-ZobmzumZOIgNX/ZiejHZQVzEbsXz30xLIRkTbZOW8Urcgm3iWGZPSsLq27N7/LE2+0/UiFR+TIbqX5HRJLV6nw==`.

`stream@0.0.3` is a useful negative control: its browser field maps `emitter`
to the undeclared `emitter-component`, but the shipped entry actually imports
the separately declared `component-emitter`, so the mapping alone does not
establish a missing dependency.

## Ambient NodeJS namespace control

`consola@3.4.2`, present in the Nuxt dependency graph, references
`NodeJS.WriteStream` in its published declarations and lists `@types/node` only
as a development dependency. In a hoisting-disabled global-store consumer,
TypeScript 5.9.3 compiled an import of consola when the consumer directly
installed `@types/node@22.14.1`, without a package extension. Removing that
consumer dependency and adding `@types/node` as a dependency of consola through
`packageExtensions` did **not** fix the `Cannot find namespace 'NodeJS'`
errors. With the types package installed in the consumer, setting `types: []`
still produced those errors; `types: ["node"]` and no extension passed. The
ambient namespace is controlled by the consumer's type inclusion, so its
presence alone is not evidence that linking a package dependency will repair a
strict-store failure. Tarball: `https://registry.npmjs.org/consola/-/consola-3.4.2.tgz`,
integrity `sha512-5IKcdX0nnYavi6G7TtOhwkYzyjfJlatbjMjuLSfE2kYT5pMDOilZ4OvMhi637CcDICTmz3wARPoyhqyX1Y+XvA==`.

Follow-up isolated Nuxt global-store runs passed `dependency.test.ts` (18
checks), `app-config.test.ts` (3) and `fs-deny.test.ts` (2), both separately and
together. The same three `jiti-fallback.spec.ts` assertion failures occurred in both local
and global layouts and reproduced alone under the global store: the assertion expects `add -D jiti`, while the generated message includes
`add --ignore-workspace-root-check -D jiti`. Tracing that focused run observed
Vite's optional `bufferutil` and `fsevents` resolution attempts; neither was
shown to cause the assertion failures. No missing registry dependency was
confirmed for these failures, no safe automatic fix was identified, and the
broad-run failure mechanism remains unconfirmed.

## A declaration that cannot be safely regenerated from shipped JavaScript

`is-bun-module@2.0.0` publishes `dist/shared.d.ts` with variable and object
initializers in an ambient declaration file. A global-store consumer importing
`isBunModule` under TypeScript 5.9.3 produced TS1046 and TS1039 diagnostics.
The package's published declaration exports the type-only names `BunVersion`
and `Modules`. Running TypeScript declaration emit on the adjacent shipped
`shared.js` generated only an `any` CommonJS export, without those contracts.
That JavaScript-derived output is not a safe repair. Xray's regeneration dry
run exited incomplete and did not propose a declaration patch. The pinned archive is
`https://registry.npmjs.org/is-bun-module/-/is-bun-module-2.0.0.tgz`, integrity
`sha512-gNCGbnnnnFAUGKeZ9PdbyeGYJqewpmc2aKHUEMO5nQPWU9lOmv7jcmQIv+qHD8fXW6W7qfuCwX4rY9LNRjXrkQ==`.

A follow-up compiled the unchanged malformed declaration as a virtual TypeScript
implementation and emitted declarations with TypeScript 5.9.3 in strict
NodeNext mode. Applying that output as a manual pnpm patch made a strict consumer
pass, including a positive `BunVersion` assignment and an expected error for an
invalid version string. The compiler reported the same seven exported names;
resolved export flags and rendered value, function, and expanded type-alias
signatures matched for this package. This is evidence for a package-specific
candidate, not a proof that declaration text can always be safely re-emitted as
implementation or that rendered type strings are a complete API equivalence test.

## Rollup plugins workspace

At `rollup/plugins` commit `b09437adcd7010c2a6cd20c778be0e4626f640f3`,
the checkout selected Node 20.20.2 and pnpm 9.5.0. Its `.npmrc` enables public
hoisting, so the local strict install explicitly disabled both hoisting settings:

```sh
CI=true pnpm install --frozen-lockfile --ignore-scripts --hoist=false --shamefully-hoist=false
```

The installed modules state recorded `hoistedDependencies: {}`. Building
`packages/commonjs` with its local Rollup binary succeeded. Running the
package's Vitest configuration directly gave 242 passing tests and one failed
suite: `packages/commonjs/test/form.js` directly requires `acorn`, which that
workspace package does not declare. This is a workspace test dependency, not a
confirmed defect in an npm registry package.

To test the global virtual store, the disposable checkout's root
`packageManager` was changed to pnpm 12.16.1, and a frozen install used
`--ignore-scripts --config.hoist=false
--config.enable-global-virtual-store=true --config.confirm-modules-purge=false`.
The modules state again recorded no hoisted dependencies, and the Vitest
symlink resolved into the global store's `links` directory. The same build
passed and the same CommonJS suite failed on `acorn`. A Node 24 xray trace of
that focused run preserved the failing exit status and reported unrelated
optional lookups; the missing `acorn` belonged to workspace test code, which
was not a candidate for a registry package extension. Adding `acorn@8.8.0` to
the disposable CommonJS workspace package's dev dependencies and reinstalling
made all 291 tests pass across four files.

A second focused `packages/node-resolve` build passed under the global store;
its tests had one failure because the workspace test requested
`@babel/preset-env` without declaring it. Adding version 7.19.4 to that
workspace package's dev dependencies made all 137 tests pass across 13 files.
Both additions were only made in the disposable checkout. The non-frozen
reinstalls re-resolved portions of the dependency graph, so the passing results
are useful diagnosis rather than controlled single-variable proofs. No registry
package repair was confirmed in this experiment, and no workspace changes were
submitted upstream.

The Rollup workspace's full `--fix --dry-run` also reported an incomplete scan
for the published `v8-compile-cache-lib@3.0.1` declaration, which ends with a
stray `x` after its valid exported `install` signature. The same bytes were
present in the registry tarball, and a separate strict global-store TypeScript
consumer reproduced TS1036 and TS2304. Version 3.0.0 also contains the stray
character. Xray's declaration-regeneration dry run did not propose a patch. A
manually reviewed pnpm patch deleting only the final character preserved the
remaining declaration text and made that consumer typecheck after a scripts-
disabled reinstall. This validates that specific human patch, not a general
rule for automatically deleting malformed declaration text. Tarball:
`https://registry.npmjs.org/v8-compile-cache-lib/-/v8-compile-cache-lib-3.0.1.tgz`,
integrity `sha512-wa7YjyUGfNZngI/vtK0UHAN+lgDCxBPCylVXGp0zu59Fz5aiGtNXaq3DhIov063MorB+VfufLh3JlF2KdTK3xg==`.

## Next.js: focused ESLint workspace and declaration probe

Next.js checkout `fac5fc87df2b60169189ac6dcba3142854124371` was tested in
separate disposable worktrees with Node 20.20.2 and pnpm 12.9.1. Both installs
used the unmodified repository security settings, the frozen lockfile, scripts
disabled, `--filter 'eslint-config-next...'`, and `--config.hoist=false`.
One used `--config.enable-global-virtual-store=false`, the other `true`. The
filter included two of 38 workspace projects, plus the root dependencies pnpm
installed. Package realpaths confirmed the two different store locations.

Initially the config package's `types` script failed because the linked
`@next/eslint-plugin-next` workspace had not been built with scripts disabled.
After installing the workspace dependency closure and explicitly building that
workspace (`pnpm --filter @next/eslint-plugin-next run build`), its build and
`pnpm --filter eslint-config-next run types` both passed in each layout. A
focused ESLint API check using the built workspace plugin also passed in both:
its `no-typos` rule flagged `getStaticPathss`. The repository's focused Jest
command could not start because the much larger `packages/next/dist/build/jest`
workspace build was absent; that is a workspace setup limitation, not an npm
package failure. These checks do not establish that the full Next.js build or
full test suite works.

The strict declaration probe imported the *installed registry package*
`@eslint-community/eslint-utils@4.9.1` from the plugin workspace and ran:

```sh
./node_modules/.bin/tsc --ignoreConfig --noEmit --module nodenext \\
  --moduleResolution nodenext --target es2022 --types node \\
  packages/eslint-plugin-next/xray-probe.ts
```

The probe was `import * as utils from '@eslint-community/eslint-utils'; void utils;`.
With `skipLibCheck` not enabled, both store layouts failed with TS2307 for
`estree` from that published package's `index.d.ts`. Xray's package scan
reported its dev-only `@types/estree` reference and produced an exact
`@eslint-community/eslint-utils@4.9.1` extension adding `@types/estree` with
the publisher's `^1.0.7` range. In each disposable worktree that generated
entry was merged into the existing `packageExtensions`, then the same filtered
install was repeated with scripts disabled and `--no-frozen-lockfile` to update
the lockfile. The identical probe then passed in both layouts. This proves the
specific declaration resolution and repair, not the usability of every xray
finding or the entire project.

Before the targeted extension, project-wide JSON scans reported 2,187 packages
and 910 findings in 400 package reports for the local store, versus 2,188 and
the same finding/report counts for the global store. Both reported incomplete
analysis with eight diagnostics, including malformed declarations in
`is-bun-module@2.0.0`, non-UTF-8 source in `faker@5.5.3`, and stylesheet and
Flow parse diagnostics. These are raw observations, not confirmed package
failures or verified fixes; some entries are shipped test or tooling files.

After parser compatibility changes, the two Next worktrees were restored to the
checkout's original workspace settings and lockfile and reinstalled frozen with
scripts disabled and the same layout flags. Repeating the scans reduced the
diagnostic count from eight to three in each layout. The remaining diagnostics
were `is-bun-module@2.0.0`'s malformed published declaration,
`lost@8.3.1`'s documentation Sass file with Jekyll front matter, and
`protobufjs@6.11.2`'s JavaScript wrapper template containing placeholders.
The latter two are tooling or documentation inputs, not confirmed executable
consumer failures. Both scans remained incomplete. The local/global package
counts remained 2,187/2,188, with 400 package reports and 910 raw findings in
each; comparing package, dependency and origin sets showed no change. This is
a reduction in parse diagnostics, not evidence of improved precision.

## Next.js asset diagnostics and an entry-only scope experiment

Two of the three remaining Next.js scan diagnostics were investigated in the
published tarballs. `lost@8.3.1` has no `exports` map, declares `lost.js` as
its main entry, and that entry loads its PostCSS implementation from `lib/`.
The reported `docs/css/main.scss` begins with Jekyll `---` front matter and
is documentation input; it is not imported by that main entry. Because there
is no exports map, consumers can still address arbitrary shipped subpaths, so
this observation does **not** prove every consumer or tool ignores that file.
`protobufjs@6.11.2`'s `cli/wrappers/es6.js` contains literal `$DEPENDENCY` and
`$OUTPUT` placeholders. Its CLI `cli/util.js` reads wrapper files as text with
`fs.readFileSync` and substitutes those placeholders. It is a CLI template, not
a JavaScript module imported by that path. The template is nevertheless used as
data by the CLI; it should not simply be discarded as unused content.

In an isolated two-package probe pointing at those installed packages, ordinary
`--json` and `--json --all-files` both reported the two parse diagnostics.
`--fix --dry-run` excluded the protobuf wrapper from its entry graph, but still
reported the `lost` documentation stylesheet: for packages without an exports
map, the repair graph intentionally treats all stylesheets as potentially public
subpaths. Other installed dependencies were reachable in this probe; its raw
finding count is not a two-package precision measure.

To evaluate a possible scope change **without changing xray's source**, a local
temporary build used the existing non-strict repair entry graph for default
read-only analysis. Explicit `--all-files` behavior was left unchanged. On the
frozen twelve-package label set, both current and experimental defaults detected
all ten reviewed positive references with zero false positives on the six clean
controls; unknown extra findings dropped from three to one. Both were complete.
The development corpus passed its expected assertions under this variant; it
lost five findings from shipped JSS test files, one from a Material test file,
one from an AJV test helper, and 30 Docusaurus findings, most involving
Docusaurus-generated or theme aliases. These labels are not exhaustive. On the
same restored Next local-store graph, the variant reported 256 raw findings in
175 package reports rather than 910 in 400, and two diagnostics instead of
three: the protobuf template disappeared, but `lost` and `is-bun-module`
remained. These counts measure a scope reduction, not improved precision.

There is a concrete cost to making that graph the default for packages without
an exports map. `@docusaurus/core@3.10.2` publishes
`lib/client/ClientLifecyclesDispatcher.d.ts`, which imports undeclared `history`
and is used by its `PendingNavigation.js` client module. A strict global-store
consumer importing the public deep path
`@docusaurus/core/lib/client/ClientLifecyclesDispatcher.js` produced TS2307 for
`history`, even though the consumer installed `history@5.3.0`. Adding an
optional `history` peer extension removed that particular error after reinstall;
an unrelated missing `@docusaurus/types` error remained. Current default analysis
reports `history`, while the experimental entry-only default misses it. This
is a real limitation of using the main-entry graph as a completeness claim for
unencapsulated packages.

A safer direction is to retain the public deep-path analysis and represent an
explicit, user-selected entry-only scope as incomplete coverage, while improving
format-aware parsing for recognized asset syntax. For example, a Sass parser
could recognize a bounded Jekyll front-matter block while preserving source
locations. Template handling would need evidence that the content is a template
or a user-specified rule; a `docs` or `templates` path alone is not proof that
a consumer cannot use it. Unknown malformed files should continue to surface
rather than silently making a package clean. Any change needs tests for public
subpaths, wildcard exports, bins and declaration companions; `--all-files` should
continue to expose assets and diagnostics.
