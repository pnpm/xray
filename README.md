# `@pnpm/xray`

Sees inside the packages you installed, and reports the dependencies they use
but never declared.

A package that imports something it does not list in its `package.json` still
works under a hoisted `node_modules`, because the import resolves by accident
against a sibling that some other package pulled in. Under pnpm's strict layout —
and especially under a global virtual store, where package directories live
outside the project entirely — the accident stops happening and the import
breaks.

The type-only cases are the ones nothing else catches. A `.d.ts` that references
a package the publisher forgot to declare produces no runtime error to observe,
because no code runs: `tsc` simply reports a missing type. Runtime detection
cannot find those, and every existing tool of this shape (`depcheck`, `knip`,
`@yarnpkg/doctor`) analyzes the source *you* wrote, not the packages you
installed.

## Install

Run it without installing anything:

```sh
pnx @pnpm/xray
```

Or add it to a project:

```sh
pnpm add -D @pnpm/xray
```

The binary is prebuilt. `@pnpm/xray` carries no code of its own beyond a
launcher — the executable travels in a `@pnpm/xray.<platform>` package that your
package manager installs only if it matches the machine.
The GNU/Linux binaries target glibc 2.28 or newer; separate musl binaries are
provided for musl-based distributions.

## Usage

Scan the dependency packages installed in a project:

```sh
xray
xray /path/to/project
```

Project scans traverse workspace links to find dependencies, but skip authored
workspace packages. When invoked from a member, repairs use the nearest workspace
settings file rather than creating a nested workspace. Scan authored packages
explicitly if needed:

Scan a single package directory:

```sh
xray --package ./node_modules/some-package
```

Output the findings as JSON:

```sh
xray --json
xray --check --json
```

JSON uses a versioned envelope with `packages`, structured `diagnostics`,
`packagesScanned`, and `complete`. A scan with parse or read diagnostics exits 2
and sets `complete: false`, even when no dependency findings were produced.
A complete scan exits 0; `--check` makes it exit 1 when findings remain. Other
command/configuration errors exit 1. Trace mode
preserves the traced command's exit code instead.

## Observe computed imports

Run a failing command through Node's resolution hooks (Node 22.15 or newer):

```sh
xray --trace -- pnpm test
xray /path/to/project --trace -- node reproduce.cjs
```

The command runs in the selected project and keeps its output and exit status.
Xray reports failed resolutions from installed packages on stderr, including
computed imports a static scan cannot name. Some failures are caught optional
integrations, so tracing alone does not change workspace settings. To select
repairs only for observed resolutions, combine it with `--fix`:

```sh
xray --trace --fix -- pnpm test
pnpm install
pnpm test
```

This preserves the original command failure status and links installed providers
for the observed references; missing optional providers stay unselected. Workspace
source failures are reported as diagnostics so the owning manifest can be corrected;
they do not generate package extensions. Explicit resolver search paths and
already-declared dependencies also produce diagnostics instead of caller repairs.
Custom loaders or
processes that replace `NODE_OPTIONS` can bypass the hook. Runtime observations
include a filename and specifier; static findings also have line and column.

## Fixing what it finds

To verify repairs against a failing, repeatable command:

```sh
xray --verify -- pnpm test
xray --verify --regenerate-types -- pnpm exec tsc --noEmit
```

Xray first runs the command unchanged. If it fails, xray tries individual
repairs, runs `pnpm install` under the existing project policies, and reruns
the command. It checks each retained proposal for necessity and verifies the
final installed graph again. An already successful command leaves settings
unchanged. The compiler can also identify a missing installed `@types` provider
when a package already declares its untyped runtime dependency.

Verification requires existing pnpm lockfiles. Unsuccessful searches restore
workspace settings, root and importer locks, generated patches, and the locked
dependency graph, returning the original command's failure status. User edits
to managed files are preserved as conflicts. Interruptions stop the command's
process group before rollback and return status 130. Processes which detach
from that group require their own cleanup.

The command may run repeatedly. Installation follows existing lifecycle-script
and trust policies; command and lifecycle outputs outside the managed files
remain. Search defaults to 32 proposals and 32 verification attempts; adjust
`--verify-max-candidates` and `--verify-max-attempts` to change those limits.

A recovery journal is persisted before mutation. If rollback encounters an edit
or installation failure, xray retains that journal and prints its location:

```sh
xray /path/to/workspace --recover /path/to/xray-recovery-directory
```

Recovery checks recorded file states before replacing anything and refuses to
overwrite intervening edits. A crash during an uncheckpointed write can require
manual reconciliation using the saved original bytes. Journals cannot be
recovered while an active verification process holds their lease.

Preview selected repairs, apply them, then let pnpm update its lockfile and links:

```sh
xray --fix --dry-run
xray --fix
pnpm install
```

`--fix` merges version-specific extensions into `pnpm-workspace.yaml`, preserving
comments, unrelated settings and existing extensions. Conflicting settings are
reported without writing the file. Running it twice does not duplicate entries.
It scans declared entry points and their imports, including declarations and
public stylesheet subpaths of packages without exports maps; use
`--fix --all-files` for deep imports outside that graph. A parse failure in that
surface excludes that entire package from repair and makes xray exit with code 2.
Independent packages can still be repaired; excluded packages are listed on stderr.
Static `require.resolve` analysis follows default lookup and the local
`{ paths: [__dirname] }` form. Foreign or opaque search roots require runtime
diagnosis because linking a dependency into the caller might not affect that lookup.
Created loaders are attributed to the caller only when anchored to its
`import.meta.url` or unshadowed `__filename`.

The planner reuses installed providers as optional peers, including type providers,
so a publisher's historical development range does not pull in an incompatible
second type tree. For absent type dependencies it uses portable author development
ranges. An untyped package's imports can be satisfied by an installed `@types`
package; when its version is unambiguous, the repair selects that exact type package.
Installed npm aliases are eligible providers when their target identity and
version match the workspace importer's declaration.
Findings without a selected repair remain visible in the scan output.

Print the selected block instead of applying it:

```sh
xray --package-extensions
```

A standalone `--package` scan has no project provider context and emits all
candidate extensions for review. A typical type dependency repair is:

```yaml
packageExtensions:
  '@medplum/core@5.1.15':
    dependencies:
      '@medplum/fhirtypes': '5.1.15'
```

Adding that to `pnpm-workspace.yaml` makes pnpm link the dependency inside the
package's own directory, so it resolves for every tool — the compiler, bundlers
and Node alike — rather than only for the ones that read `tsconfig.json`.

The selector is pinned to the affected package version. Findings from multiple
installations of that version are combined. Review the suggestions: references
can be optional integrations or build artifacts. Portable publisher ranges are
used to install missing type dependencies. Runtime imports remain optional
peers because a publisher may support many optional integrations. Local paths and workspace protocols
cannot be reused outside the publisher's checkout.

When no portable range is available, the output uses an optional peer with range
`'*'`. This links a dependency only when it is already available; it does not
install one. Those suggestions require review and a suitable version supplied by
the consumer.

## Repair malformed declarations

When a published declaration is syntactically broken and matching JavaScript is
shipped, try a compiler-verified pnpm patch:

```sh
xray --fix --regenerate-types --dry-run
xray --fix --regenerate-types
pnpm install
```

This requires a local TypeScript installation and `tsconfig.json` in the target
project. Xray emits declarations from the shipped JavaScript without executing
it. For declaration files containing implementation initializers, it can instead
re-emit the original text as virtual TypeScript source, preserving declaration-only
types. It can also remove a final unresolved bare identifier when TypeScript
confirms that this statement is invalid and every valid declaration stays unchanged.
It checks the existing export contract and verifies the project in memory with
all declaration checking enabled. A candidate is rejected if any diagnostics
remain; the reason is printed. Source files in the package store are untouched.

Verified changes are saved as reviewable files under `patches/` and referenced
by `patchedDependencies`. An initial incomplete scan still exits 2 until the
patch is installed. Regeneration is intentionally opt-in: it cannot infer a
correct declaration from every broken package or reconcile every compiler and
type-provider version. Composite project references are checked in their own
compiler contexts, including every transitive project and the repaired outputs
seen by dependents. Missing, stale, or out-of-project build outputs require a
rebuild before regeneration can be verified.

## What it reports

Findings come in two kinds:

- **declared as a devDependency** — the package build-depends on it and ships
  references to it. The publisher knew about the dependency and expects consumers
  to supply it, which makes these the strongest candidates for a `packageExtensions`
  entry.
- **not in the manifest at all** — the name appears nowhere in the manifest. More
  often a bundling artifact or an optional integration, so these deserve a look
  before you act on them.

Three things reliably produce findings that are true of a file but not of the
package: bundled output that still names the modules it inlined, generator
templates describing what a *generated* project needs, and tests that ship in
the tarball. Reading the origin and the severity together usually separates
them.

A package that declares an `exports` map has said which of its files a consumer
may reach, and only those files and what they import are read. Without such a
map `require("pkg/test/spec")` resolves, so every shipped file is part of the
package's surface whether its author meant that or not, and every one is read.
`--all-files` ignores the map and reads everything regardless.

## What it looks at

Every file a package ships as behaviour or as types — `.js`, `.mjs`, `.cjs`,
`.jsx`, `.ts`, `.mts`, `.cts`, `.tsx` and the `.d.ts` family — parsed with
[oxc](https://oxc.rs). Flow-marked files use the bundled official Flow parser
through Node when their syntax is unsupported by oxc. Every position that names a module counts: `import` and
`export … from`, `export *`, `require()` and `require.resolve()`, dynamic
`import()`, type-position `import("pkg")`, `import x = require("pkg")`, and
`/// <reference types="pkg" />`.

Findings include package-relative filenames, one-based line and column numbers,
the original import specifier, and whether each reference is code or types. A dependency reached from executable
code breaks the program when it is missing. One reached only from declarations
breaks type checking instead — which is why no runtime detector can see it, and
why Yarn's Plug'n'Play, which finds undeclared dependencies by failing at
runtime, has no entry for most of them.

Parsing rather than pattern matching is what keeps the results honest — a
specifier mentioned in a JSDoc example is not a dependency, and a scanner built
on regular expressions cannot tell the difference.

Stylesheet dependencies count too: CSS, SCSS, Sass and Less `@import`, `@use` and
`@forward` rules are parsed with [raffia](https://github.com/g-plane/raffia).
Local Sass partials, index files and import-only files are followed before a bare
name is treated as an external package. Builtin `sass:` modules, remote URLs and
interpolated paths are excluded. Bare CSS names can also be browser-relative
URLs, so automatic repairs require an installed provider.

A specifier is not reported when nothing has to be installed for it to resolve:
relative paths, subpath imports, protocol URLs and Node builtins. A
`/// <reference types="x" />` is satisfied by `@types/x` or by `x` itself, and a
bare `import` of `x` is satisfied by `@types/x` when that package declares the
module ambiently.

Files that fail to parse are reported on stderr rather than passed over, so a
package is never quietly recorded as clean because its declarations could not be
read.

## License

MIT
