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

## Usage

Scan everything installed in a project:

```sh
xray
xray /path/to/project
```

Scan a single package directory:

```sh
xray --package ./node_modules/some-package
```

Output the findings as JSON:

```sh
xray --json
```

## Observe computed imports

Run a failing command through Node's resolution hooks (Node 22.15 or newer):

```sh
xray --trace -- pnpm test
xray /path/to/project --trace -- node reproduce.cjs
```

The command runs in the selected project and keeps its output and exit status.
Xray reports failed resolutions from installed packages on stderr, including
computed imports a static scan cannot name. Some failures are caught optional
integrations, so these observations do not automatically become fixes. Workspace
source failures and already-declared dependencies are excluded. Custom loaders or
processes that replace `NODE_OPTIONS` can bypass the hook. Runtime observations
include a filename and specifier; static findings also have line and column.

## Fixing what it finds

Preview selected repairs, apply them, then let pnpm update its lockfile and links:

```sh
xray --fix --dry-run
xray --fix
pnpm install
```

`--fix` merges version-specific extensions into `pnpm-workspace.yaml`, preserving
comments, unrelated settings and existing extensions. Conflicting settings are
reported without writing the file. Running it twice does not duplicate entries.
It scans declared entry points and their imports, including declarations; use
`--fix --all-files` for deep imports outside that graph. A parse failure in that
surface stops the repair so incomplete evidence cannot silently become a fix.

The planner uses portable author development ranges for missing type dependencies.
It links existing runtime providers as optional peers, leaving unused integrations
uninstalled. Type-only imports can be satisfied by an installed `@types` package;
when its version is unambiguous, the repair installs that exact type package.
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
[oxc](https://oxc.rs). Every position that names a module counts: `import` and
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
