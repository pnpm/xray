# Registry corpus

This opt-in regression corpus downloads exact published npm tarballs and verifies
SHA-512 digests before scanning them. It does not install dependencies or run
package scripts. It requires Node.js 18 or newer, `tar`, network access, and a
built xray binary:

```sh
cargo build
node corpus/run.mjs
# Or choose another binary:
node corpus/run.mjs /path/to/xray
```

Broken-package entries record a shipped import and the manifest declaration that
makes it a candidate for an extension. Clean controls assert zero findings.
Most entries assert zero diagnostic lines; unenv, is-bun-module and v8-compile-cache-lib have malformed
published declarations and the corpus checks that those diagnostics remain visible. These checks do not certify that
every finding is actionable or that a suggested version is compatible.
In particular, Docusaurus uses generated and bundler aliases; treating those as
npm packages would generate false positives. Its published `.js` files also
contain JSX. The tarball checks scan individual packages and are a starting point,
not a representative measure of registry-wide coverage.

Add cases with a pinned tarball URL, verified digest, concrete evidence, and
expected findings; avoid relying on `latest` or executing untrusted package code.

## Installation regression

The separate integration test requires pnpm and installs pinned registry packages
with lifecycle scripts disabled in temporary workspaces. It runs TypeScript before
and after applying `xray --fix` in both isolated and global virtual stores with
hoisting disabled. Each run checks that dry-run does not write, existing settings
and comments survive, and repeating the fix is idempotent. After reinstall, the
root scan must stop reporting each repaired finding. It checks that the package physically lives outside
the project in the global-store case. For the declaration case, the consumer does not declare the missing dependency,
so success requires the extension to install and link it. A small ambient
declaration isolates an unrelated optional pdfmake peer. For the runtime case,
the consumer supplies pug and the generated optional peer makes it reachable
from @vue/consolidate before a template is rendered. The runtime case also
checks that `--trace` reports the failed resolution and preserves the child exit
status before repair, then succeeds after repair. The JSS case disables
pnpm’s built-in compatibility database so the original undeclared dependency
can be reproduced; its consumer supplies the missing runtime package. The
Rollup case verifies that a missing declaration resolves through the installed
`@types/estree` provider rather than a guessed `estree` package. The Material
Components case verifies undeclared Sass subpath imports by running a real Vite
build before and after the repair. The unenv case verifies opt-in declaration
regeneration with compatible TypeScript and Node types, and checks that an
incompatible Node types version produces no patch or workspace changes. The
browser mapping case checks esbuild bundling with selected host builtins external;
it does not claim a working browser runtime. Synthetic regeneration checks reject
changes to unused exported types, missing or cyclic TypeScript references, stale
reference outputs, and repairs that introduce downstream type errors. Built
project-reference graphs are verified in their separate compiler contexts.

```sh
cargo build
node corpus/install-test.mjs
```

The integration test covers seventeen known broken releases; it does not establish that
all generated fixes are safe or that every registry package has been checked.

## CLI safety checks

Run `node corpus/safety-test.mjs` to exercise conflicting extensions, parse
failures and independent-package repairs, dry-run writes, reuse of an installed modern type provider across
historical dev ranges, custom resolver ownership, workspace-source diagnostics,
and trace stream and exit-status handling with local
synthetic packages. These checks do not fetch or execute registry code.

Some candidates were discovered using the public [nubjs package-extensions
dataset](https://github.com/nubjs/package-extensions). Their tarballs and failure
conditions were independently verified here; the rest of that dataset was not.

## Discovering candidates from an installed workspace

`discover.mjs` scans an existing project, checks package names and exact versions
against public npm registry metadata, and prints a ranked **unverified** candidate
list as JSON. It does not install packages, run package scripts, fetch tarballs,
or add cases to the verified corpus. The registry requests use at most four
concurrent requests, have timeouts, and reject redirects. Local workspace package
names and absolute or escaping evidence paths are excluded from output.

```sh
cargo build --release
node corpus/discover.mjs /path/to/installed/project target/release/xray > candidates.json
node --test corpus/discover.test.mjs
```

The envelope includes scan completeness and a diagnostic count, but omits local
scanner diagnostic paths. Each candidate contains a registry tarball URL and
SHA-512 digest from metadata, not a verified archive: download and hash the
archive, examine the published manifest and relevant file, reproduce the
failure, and verify any proposed repair before promoting it to `packages.json`.
Static findings can describe optional integrations, bundled references or tests;
a registry match does not prove an installed copy has the published contents.
A failed metadata request omits that candidate, so the count is not an
exhaustive measure of the project or npm.

## Checking discovered type candidates

The opt-in verifier tries up to five eligible candidates by default, sequentially,
in temporary Linux pnpm projects. It requires Node, pnpm and a built xray binary.
It installs with lifecycle scripts disabled and existing pnpm policies in force;
it does not execute the candidate's JavaScript. Only simple publisher-declared
type dependency ranges are eligible. It installs TypeScript 5.9.3 and
`@types/node` 22.14.1 as the test consumer's tooling, with hoisting disabled and
the global virtual store enabled.

```sh
node corpus/verify-candidates.mjs candidates.json --xray target/release/xray --limit 5 > verification.json
node --test corpus/verify-candidates.test.mjs
```

`--limit` accepts 1–25. For each case, the verifier checks for the selected
missing-module diagnostic, runs xray's repair, reinstalls with scripts disabled,
then requires both a clean typecheck and a complete scan without that finding.
It reports `confirmed`, `skipped`, or `blocked`, with diagnostic codes but no
local paths or raw install logs. A `confirmed` result is evidence for that
specific generated consumer and toolchain, not a guarantee for all consumers.
Before installation, the verifier downloads the target archive from the public
registry, rejects redirects and files larger than 64 MiB, verifies its SHA-512
against the registry pin, and installs that local archive. It verifies the
installed target identity and that its real path is outside the project. Output
marks that archive `sha512_verified`; provider pins remain metadata-only.
Independently review the case and provider before adding it to the verified
corpus. The verifier never edits `packages.json`.

## Verified command loop

`node corpus/verify-loop.mjs` exercises `--verify` in isolated and global virtual
stores. It includes actual consumer commands for the declared-runtime/missing-
types case in `@vercel/sandbox` and a public Docusaurus deep declaration, in
addition to runtime, rollback, recovery, cancellation, and separate-lockfile
checks. The Docusaurus case installs a large dependency graph and can take
several minutes. These tests run the explicitly provided consumer commands;
fixture workspaces set `ignoreScripts: true` for pnpm installs.
