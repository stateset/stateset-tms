# Pinned upstream notice material

The [catalog](../../third-party/npm-notices.json) supplies missing or supplemental materials for nine exact locked npm releases. It binds archive integrity, every regular source-package file observed at preparation, material hashes and primary source URLs. Collection validates these files offline; it does not download or execute upstream scripts. The metadata check also binds each catalog entry to the current web lock.

## Material gaps addressed

Six records that previously lacked standalone license text now have version-bound text available: `@next/env`, both installed `@next/swc-*` packages, `glob-to-regexp` and both installed `@img/sharp-libvips-*` packages. Package-provided files and pinned upstream materials remain separate in the manifest; none of the installed dependency directories are rewritten.

- Next.js's own MIT text is copied from [release commit 195cd6a](https://github.com/vercel/next.js/blob/195cd6a714086ee0e442cb551da376ad71f37035/license.md), corresponding to v15.5.28. It matches the license shipped by the installed `next` package. It covers the package's own terms; it does not inventory every library embedded in SWC or other compiled assets. Native package manifests in this repository use a build-time version placeholder; the published npm version/integrity and release tag are recorded separately rather than claiming that placeholder is a release version.
- `glob-to-regexp`'s full BSD terms were already in its README. The extracted notice retains the original copyright and all conditions/disclaimer bytes. Both the installed README and its version match [published gitHead 2abf65a](https://github.com/fitzgen/glob-to-regexp/blob/2abf65a834259c6504ed3b80e85f893f8cd99127/README.md). The source README hash and extraction boundary are retained.
- The image packages declare LGPL-3.0-or-later, alone or in a combined expression. The bundle now includes the canonical [LGPLv3](https://www.gnu.org/licenses/lgpl-3.0.txt) and incorporated [GPLv3](https://www.gnu.org/licenses/gpl-3.0.txt) texts, plus the original [libvips 8.18.7 source license](https://github.com/libvips/libvips/blob/24ad4d042940e6bf99a68871ba886ca8847c9c82/LICENSE). These texts do not supply every component's copyright or complete a binary review.

The catalog explicitly distinguishes package license text, reference license text, build-repository license, component notices, build recipes, attribution and provenance. A build repository's Apache license is never substituted for a compiled library's LGPL terms. An attribution paragraph alone cannot close a missing-license-text gap. Mandatory native/complex-license review reasons are not waived by the catalog.

## Native source references

Both installed Linux libvips packages' 28 component versions match the publisher's [version properties at gitHead ebb95f8](https://github.com/lovell/sharp-libvips/blob/ebb95f8add54eee8bed840e3fb587e4cbec857d7/versions.properties). The copied source index records 34 expanded source/patch locators from the matching POSIX recipe. It is a set of references, **not a corresponding-source archive**: those source bytes have not all been retrieved or verified, and relinking is not qualified.

The pinned recipes expose limits that a binary reviewer must resolve: one patch is fetched from a mutable PR endpoint, the POSIX recipe fetches its notice table from `main`, and the WebAssembly recipe can default to `HEAD` for its delegated build. The WebAssembly package's actual component versions are bound separately, rather than being represented as identical to the Linux component set. Copies of the recipe and its own Apache license are reference material only.

Collection now inventories recognizable native filenames and ELF, Wasm, PE, archive and Mach-O signatures across installed npm packages. It records byte hashes without copying/executing binaries, refuses package-content symlinks and retains a distribution-review reason even when a package's MIT/Apache text is supplied. This also identifies native assets in `next` and the sharp bindings; package-level license labels did not expose all of those assets. The inventory remains conservative and does not establish which files a delivered artifact uses or every embedded component.

## Attribution and unresolved authorship

The prepared `caniuse-lite` attribution records the published package author credit, pinned source, license link and verified unmodified source-package state. It is copied alongside the installed CC-BY-4.0 text. A distributor must review the actual transformed artifact and indicate its own changes as appropriate; the [CC BY 4.0 terms](https://creativecommons.org/licenses/by/4.0/) remain independent of the first-party dual license.

`client-only@0.0.1` has exactly three published regular files: `package.json`, empty `index.js` and `error.js`. The archive declares MIT but provides no license file, author, repository or copyright statement. We do not infer a holder from React home/issue links or copy a Vercel/Meta copyright line onto that separate release. Its missing-text finding remains open. These observations come from the checksum-verified archive; forum claims are not used as copyright authority.

## Remaining review

The six material-text gaps are resolved under the collector's narrow policy. A broader native inventory adds review candidates, so the updated strict gate has **ten package records requiring review**: eight packages with native assets, `caniuse-lite` attribution/distribution terms and `client-only`'s missing notice. This is not a count of vulnerabilities or incompatible licenses. Source acquisition, source/build provenance, copyright completeness, binary composition, relinking, runtime/system notices and distribution approval remain separate work.

Run the [publication review procedure](publication-review.md) to collect a fresh bundle. `--fail-on-review` still exits 2. Neither the catalog nor successful collection grants confidentiality, ownership or distribution clearance; the repository remains private.
