# Contributing to rtao

Follow [PLAN.md](PLAN.md) and the [architecture](docs/plan/architecture.md).
Implementation proceeds through M0–M9, with checks before each push to main.

## Attribution and licensing

Original files use `MIT OR Apache-2.0`. Every Rust file copied or derived from
upstream, including staged code, ports and generated extracted tables, starts
with this header (substitute the exact path and full commit):

```rust
// Derived from RaoFoundation/subtensor <path> @ <commit> (Apache-2.0); modified by rtao.
```

Keep existing copyright and attribution notices. Packages containing derived
files declare `license = "(MIT OR Apache-2.0) AND Apache-2.0"`; other packages
inherit `MIT OR Apache-2.0`. The parity gate will verify this against source
headers and `parity/map.toml` when it lands in M0-13.

Vendored assets stay byte for byte, retain their notices and are listed in
NOTICE. LICENSE-APACHE supplies the upstream license text. Commit Cargo.lock;
checks use `--locked`.
