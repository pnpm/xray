Flow parser 0.266.1 is vendored from https://registry.npmjs.org/flow-parser/-/flow-parser-0.266.1.tgz.
The tarball SHA-512 is 74e37a87ec8eec519afc53b9350099b991cdd28dc8db712fe9560e25ff5df0ba5dac0ac73eddfdc04fbe2cb9ae78d57f84d639860586208ae09eb0e445c5e43e.
LICENSE is from https://github.com/facebook/flow/blob/v0.266.1/LICENSE.

`SHA512SUMS` records the SHA-512 of `flow_parser.js` as extracted from that
tarball, and CI checks the vendored file against it. When updating the parser,
verify the tarball against the SHA-512 above (or the registry's integrity
field), copy `package/flow_parser.js` here, and regenerate the record with
`sha512sum flow_parser.js > SHA512SUMS`.
