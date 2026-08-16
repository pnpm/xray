# Format every file the linters check.
fmt:
  cargo fmt
  taplo format

# Type-check without producing a binary.
check:
  cargo check --locked --all-targets

test:
  cargo test --locked
  node --test npm/xray/test/*.test.mjs

lint:
  cargo clippy --locked --all-targets -- --deny warnings

dylint:
  env RUSTFLAGS="-D warnings" cargo dylint --all -- --all-targets

# Rustdoc is a gate, not just output: broken intra-doc links fail the build.
doc:
  env RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items

# Everything CI runs, in the order that fails fastest.
ready:
  typos
  just fmt
  just check
  just test
  just lint
  just dylint
  just doc
  cargo deny check
  git status
