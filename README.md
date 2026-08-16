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

## Fixing what it finds

`--package-extensions` emits a ready-to-paste block that declares each finding as
an optional peer dependency:

```sh
xray --package-extensions
```

```yaml
packageExtensions:
  '@medplum/core':
    peerDependencies:
      '@medplum/fhirtypes': '*'
    peerDependenciesMeta:
      '@medplum/fhirtypes':
        optional: true
```

Adding that to `pnpm-workspace.yaml` makes pnpm link the dependency inside the
package's own directory, so it resolves for every tool — the compiler, bundlers
and Node alike — rather than only for the ones that read `tsconfig.json`.

The range is `'*'` on purpose. The goal is to make a package that is already
installed reachable, not to constrain which version gets picked; a narrower range
would only produce unmet-peer warnings without preventing anything.

## What it reports

Findings come in two kinds:

- **declared as a devDependency** — the package build-depends on it and ships
  references to it. The publisher knew about the dependency and expects consumers
  to supply it, which makes these the strongest candidates for a `packageExtensions`
  entry.
- **not in the manifest at all** — the name appears nowhere in the manifest. More
  often a bundling artifact or an optional integration, so these deserve a look
  before you act on them.

## What it looks at

Shipped `.d.ts`, `.d.mts` and `.d.cts` files, parsed with
[oxc](https://oxc.rs). Every position that names a module counts: `import` and
`export … from`, `export *`, dynamic `import()`, type-position `import("pkg")`,
`import x = require("pkg")`, and `/// <reference types="pkg" />`.

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
