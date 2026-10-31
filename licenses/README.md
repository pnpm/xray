# Rust license notices

`THIRD_PARTY_NOTICES` contains the runtime dependency license texts for the
union of the eight targets in `about.toml`. The wrapper and each native npm
package carry the same file. Build-only and test-only dependencies are not
included in that union. The union may include a dependency on more targets
than any one executable actually uses.

To regenerate after updating `Cargo.lock`:

```sh
cargo install --locked cargo-about --version '=0.9.3' --features cli
cargo fetch --locked
node scripts/generate-rust-notices.mjs
node scripts/generate-rust-notices.mjs --check
```

The generation and check run `cargo-about generate --frozen --fail` offline;
`cargo fetch --locked` prepares all locked platform sources first. CI checks
the committed output with the same pinned tool version. Do not hand-edit the
Rust section of `THIRD_PARTY_NOTICES`.

Some published crates omit license files or use names that cargo-about does not
recognize. Its fallback is a generic SPDX text and may omit copyright notices.
`rust-overrides.json` maps each reviewed fallback to a license file under
`rust-overrides/`, the exact upstream revision and SHA-256 of that file. The
generator checks each hash and fails on new or stale fallback entries. Review
the published crate and the source at its `.cargo_vcs_info.json` revision
before adding an override. For objc2 and siphasher, the upstream file
identifies the MIT license choice but does not contain its full terms, so the
generator also includes the generic MIT terms without inventing an attribution.
For yaml-edit 0.3.5, the published manifest specifies Apache-2.0 and its pinned
upstream revision has no license or copyright file; the generic Apache text is
included. These cases must be revisited if the crate version changes.
