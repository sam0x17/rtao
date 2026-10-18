# rtao book

This directory holds the authored sources of the rtao user guide. Several
pages linked from `src/SUMMARY.md` (for example `aliases.md`,
`divergences.md`, `compat.md`, and the transaction, query and error
references) are generated from the pinned upstream inputs and the compiled
examples, so they do not exist in `src/`. Links to them resolve only in the
built book; running `mdbook` directly on this directory produces an incomplete
book with broken links.

Generate those pages with `cargo xtask docs`, then render the combined tree:

```sh
mkdir -p <out>/generated
cargo xtask docs --llms --out <out>/generated
cp -R book <out>/book-source
cp -R <out>/generated/. <out>/book-source/src/
mdbook build <out>/book-source --dest-dir <out>/book-html
```

`ci/docs-check.sh` is the authoritative recipe. Hosted CI runs it with a
pinned mdBook release and retains the rendered book and Rust API docs as a
downloadable artifact.
