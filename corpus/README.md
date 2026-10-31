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
Most entries assert zero diagnostic lines; the JSS release has a known Flow
parsing warning. These checks do not certify that
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
and comments survive, and repeating the fix is idempotent. It checks that the package physically lives outside
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
`@types/estree` provider rather than a guessed `estree` package.

```sh
cargo build
node corpus/install-test.mjs
```

The integration test covers five known broken releases; it does not establish that
all generated fixes are safe or that every registry package has been checked.
