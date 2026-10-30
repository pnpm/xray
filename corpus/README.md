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

Each entry records an observed shipped import and the manifest declaration that
makes it a candidate for an extension. Assertions are deliberately limited to
positive findings: they do not certify that every finding is actionable, that a
suggested version is compatible, or that the emitted extension fixes a project.
In particular, Docusaurus uses generated and bundler aliases; treating those as
npm packages would generate false positives. Its published `.js` files also
contain JSX, which some parser configurations reject. The corpus currently scans
individual unpacked packages, not full monorepo installs under strict hoisting
or a global virtual store. It is a starting point, not a representative measure
of registry-wide coverage.

Add cases with a pinned tarball URL, verified digest, concrete evidence, and
expected findings; avoid relying on `latest` or executing untrusted package code.
