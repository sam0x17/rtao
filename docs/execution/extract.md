# M0 literal extraction

`cargo xtask extract` reads the vendored
`sdk/python/tests/unit/test_http_auth.py` and generates
`vendor/upstream/extracted/btauth.json`. `--check` re-extracts and compares the
entire file byte for byte without writing. Neither command executes Python or
needs the network. The input must match its git blob in the watched tree;
`import-upstream --verify-only` separately authenticates the committed tree and
source inventory against `upstream.toml`.

The format has a version, source tag/commit/path/blob, and named values with
`line` and `end_line`. Booleans, null, strings, integers and finite floats use
ordinary JSON literals. Integers retain their exact decimal tokens through
`serde_json/arbitrary_precision`. Bytes use `{"bytes_hex":"..."}`, lists use
arrays, tuples use `{"tuple":[...]}`, and dictionaries use
`{"dict":[[key,value],...]}` to preserve non-string keys and insertion order.
Sets and frozen sets use `{"set":[...]}` and `{"frozenset":[...]}` with
canonical JSON ordering. Numerically equal Python keys collapse without
rounding large integers through floats.

The v0 subset accepts prior module-level literal names, implicit string/bytes
concatenation, unary numeric signs, bounded constant folding, and the builtin
constructors `dict`, `list`, `tuple`, `set`, `frozenset` and `bytes`. Shadowed
constructors, comprehensions, arbitrary calls, f-strings, unpacking, mutations,
module-level control flow and unsupported module assignments fail with source
location. In particular, an unsupported assignment cannot silently change a
previously resolved name. Set iteration into an ordered container is refused.
Function defaults are checked as literals, assignment expressions in decorators
are refused, and v0 rejects class definitions. The remaining resolved forms and
additional tables belong to M1/M2.

Extraction limits source bytes, aggregate allocation, container width, and
recursive expression/value depth. Folded integers satisfy `|value| < 2^256`,
and string folding checks its output size before allocating. Resolved names
spend the same allocation budget before cloning, so repeated container aliases
cannot bypass the bounds.

Validation on 2026-10-14 at parity tag `v478`, commit
`850f2755bb05a3851f948489f0ab27b0645cebce`:

- `cargo test -p xtask extract::tests --locked`: seven tests pass, covering
  accepted/refused literals, exact integers and bytes, Python numeric-key
  equality, mutation and alias refusal, resource bounds, determinism, stale
  output, and tampered source.
- `cargo xtask extract --check`: the committed output matches all six source
  goldens at blob `0a2fdf4710f4c6a3868aad7f300414775e218d71`.
- `cargo clippy -p xtask --all-targets --locked -- -D warnings`: passes.

Temporary builds and test fixtures used only the recorded task root
`target/execute-plan.6Ro0PC/extract`. Tests start no processes or configuration
dependencies and remove their fixture directories on return. Disposable build
outputs are removed after validation; the source, generated goldens and this
bounded evidence remain in the repository.
