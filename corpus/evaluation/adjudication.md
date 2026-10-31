# Review of unscored holdout findings

This review is separate from the frozen labels and does not change the reported
holdout metrics. It uses the published tarballs and versions in `frozen.json`.
The scanner's all-files results include shipped tests as well as consumer entry
points, so a source reference alone does not prove a repair is needed by consumers.

| Package and finding | Source evidence | Assessment |
| --- | --- | --- |
| `@angular-eslint/template-parser@22.6.0` → `@typescript-eslint/utils` | `dist/index.d.ts:4` imports `./convert-source-span-to-loc`, and `dist/convert-source-span-to-loc.d.ts:2` imports types from `@typescript-eslint/utils`. The manifest specifies `dist/index.d.ts` as `types` and does not declare `@typescript-eslint/utils` in dependencies or peers. | Real undeclared type dependency on the published declaration graph. This is additional evidence of a package issue, though it was not among the original frozen labels. |
| `@jimp/jpeg@0.22.12` → `@jimp/test-utils` | The only source import is `test/jpeg.test.js:1`; the manifest lists it in `devDependencies`. Published runtime entries are `dist/index.js` and `es/index.js`, and the type entry is `index.d.ts`. | A real test import, but the reviewed evidence does not establish a missing consumer dependency. Installing it for ordinary consumers is not justified by this test reference. |
| `@jimp/jpeg@0.22.12` → `@storybook/expect` | The only source import is `test/jpeg.test.js:3`; it is absent from the package manifest's dependency sections. | An undeclared import in a shipped test. The reviewed evidence does not establish that consumer entry points need it, so an automatic consumer repair is not justified on this evidence. |

The first finding's impact depends on the consumer's TypeScript resolution and
installed graph. The last two findings remain valid observations for an explicit
all-files audit; this review does not relabel them as scanner errors.
