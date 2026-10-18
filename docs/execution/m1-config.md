# Shared btcli configuration and display caches

`rtao_client::config` ports the pinned `sdk/python/bittensor/config.py`
configuration and cache behavior for M1-11. Its seven paths, environment-variable
names, settable type tags and refresh intervals come from generated
`rtao_catalog::config` exports. The library accepts an injected home, environment
reader and Unix clock; tests do not change the process environment or access the
user's home directory.

The async `Store` moves each complete read or mutation onto the blocking pool.
Writes create a unique adjacent temporary file, sync its contents, replace the
destination and sync the parent directory. Failed replacements remove the
temporary file. Invalid JSON moves to `.corrupt.<seconds>`; another malformed
file at the same timestamp receives a distinct backup suffix. Cross-process
updates retain Python's last-writer-wins behavior.

Config loading ignores unknown keys and preserves known JSON values exactly.
Setting values uses the extracted string or boolean tags. Boolean spellings and
errors follow Python, and removing a stored null value returns false as Python
does. Cache entries preserve other networks, store integer timestamps, and use
the strict 24-hour names and seven-day symbols refresh intervals. Cache parsing
preserves Python's integer-key and string-value coercions. Raw cache keys use
`BigInt`; `CacheMap::chain_values` returns checked `NetUid` entries and an explicit
list of rejected keys for client diagnostics.

Historical validation on 2026-10-14 used the complete workspace product, without
substitute modules or local Python. Current substantial checks run in free
`ubuntu-latest` CI under Q30; the commands below require no local rerun:

```text
cargo test -p rtao-client config:: --lib --all-features --locked
11 passed; 0 failed; 0 ignored

cargo check -p rtao-client --all-features
passed

cargo +1.89.0 check -p rtao-client --no-default-features
passed

cargo clippy -p rtao-client --lib --all-features -- -D warnings
passed
```

The tests cover all paths and empty overrides, exact wide JSON integers, unknown
keys, coercion and diagnostics, corrupt-file retention, atomic replacement and
failure cleanup, network preservation, cache boundaries, normalized key
collisions, Python rendering and checked chain identifiers. A current-thread
runtime test verifies that the store's clock and filesystem operation execute on
a blocking worker.

At that checkpoint, the strict all-target Clippy attempt found only
`items_after_test_module` in `view.rs`, where the dynamic `View` implementation
followed its test module. The full library passed strict Clippy, including the
config module. Any required integrated rerun belongs in hosted CI.

All tests ran with `RTAO_TEMP_ROOT`, `CARGO_TARGET_DIR` and `TMPDIR` beneath a
recorded local task root. Test-owned configuration directories were removed
after their blocking tasks completed. No process, launchd unit, symlink or
virtual environment was created or given a dependency on those fixtures. A
post-test directory check found no remaining `tmp/config-*` fixtures. That
execution root is retired and absent; do not recreate it or resume its commands.
