# Catalog namespace generation

`xtask regen` generates the fourteen read namespace structs and the blanket
`Namespaces: View` accessor trait from the pinned `namespaces.py` category
literals and `NAMESPACES` table. The catalog category controls each accessor;
all nonempty Python read prefixes must agree. Both inputs are SHA256-verified
against `upstream.toml` before parsing.

The Rust AST parser rejects dictionary aliases, class/category rebindings,
unsupported executable module or class statements, decorated module functions,
invalid identifiers and missing or ambiguous categories. Curated methods stay
in their owning modules. The generated `Subnets` has no inherent `metagraph`
method, preserving `SubnetsMetagraphExt` resolution. The facade prelude exports
the current view/client vocabulary, namespace trait, primitive IDs/units and
metagraph extension under their corresponding features.

Current validation passes three generator tests, including mutation cases for
method/category rebinding and conditional assignment. The scoped
`regen --namespaces-only --check` check and the integrated 28 reads/metagraph
tests passed before the subsequent model-generator integration. Rust 1.89
facade compilation with `metagraph` also passed. Source-generation tests were
rerun after the added binding checks and xtask dependency integration:
three passed, none failed.

Scoped checks do not establish full regeneration or any milestone exit. Full
`regen --check`, strict integrated Clippy, platform gates and live validation
remain required after the model and fixed-point shape changes settle.

This work uses the execution's recorded temporary root. No separate disposable
directory was created; shared builds remain needed by the active goal.
