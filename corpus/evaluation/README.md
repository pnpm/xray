# Frozen, bounded evaluation

`frozen.json` was frozen on 2026-10-31, before running xray on these archives.
Its SHA-256 is `c51c93ec127e98b4b51941863376e21261ed953ad8bcf215ab119bc79fd2cdc6`;
`run.mjs` rejects any different manifest. Six packages were selected from a
public candidate list, then their pinned tarball contents and manifests were
manually inspected to label declaration imports. Six controls were selected for
simple published code, declared dependencies, and conditional exports and types;
their shipped source was inspected before labeling them clean. None was drawn
from the existing regression corpus. No labels were tuned after freezing.

```sh
cargo build
node corpus/evaluation/run.mjs > evaluation-results.json
```

The runner verifies SHA-512 of each downloaded tarball and does not execute its
code. It scores the reviewed expected dependencies as true positives or false
negatives. Findings on clean controls are false positives. Other findings in
broken packages are **unknown**, not assumed wrong; incomplete scans are also
excluded from scoring and reported. Precision and recall apply only to these
reviewed labels. A missing denominator is `null`. A separate
[adjudication](adjudication.md) reviews the three additional findings without
changing the frozen labels or reported metric.

This hand-selected sample is small and biased toward declaration references and
simple controls. It is not a random sample, does not estimate prevalence in npm,
and does not measure repair correctness, compatibility, runtime behavior, or
strict and global-store installation success. The separate installation corpus
exercises some of those behaviors but is development data, not this holdout.
