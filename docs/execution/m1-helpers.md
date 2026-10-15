# M1 pure helper evidence

The new `rtao-primitives::{drand, ratio, hyperparams}` modules port the pure
helpers from pinned v478 Python. No Python was executed locally, and this
record does not claim the M1 common exit or a differential oracle CI run.

`ratio_fraction` matches exact, case-sensitive identities and returns `None`
for unknown identities. It rounds integer division once, including inputs
above 2^53, and preserves negative or above-denominator raw values. Its table
derives the existing primitive denominators; the catalog test compares all
four entries with `vendor/upstream/extracted/named_tables.json`.

`proportion_to_raw` distinguishes raw integers from human floats and decimal
or exponent strings, rejects booleans, and uses Python's multiplication and
half-to-even rounding. Raw text uses BigInt so rejected values retain their
exact diagnostic. The result is u128: Python's `1.0 * u64::MAX` rounds to 2^64,
while a raw integer u64::MAX remains unchanged. Chain callers must check the
target width. `proportion_to_raw_with_label` supplies field-specific errors.
This does not extend Q25's native-balance divergence to proportion helpers.

Drand arithmetic uses the core's `GENESIS_TIME` and `DRAND_PERIOD` constants.
`current_round` accepts an injected Unix clock; `round_at` retains Python's
truncated-millisecond ceiling, including submillisecond behavior at a pulse.
`reveal_time` returns UTC and preserves round zero's pre-genesis meaning.
Native round overflow and Python datetime year bounds are checked.

Duration parsing covers numeric strings, Unicode decimal digits, repeated
units, arbitrary unit order, and signed/nonfinite numeric seconds. Formatting
truncates fractional seconds and uses days/hours/minutes/seconds. Moment text
supports Python's extended/basic/week dates, date-only and naive UTC,
arbitrary single Unicode separators, comma fractions, truncated microseconds,
and timezone seconds/fractions. Numeric moment strings are rejected.

The ISO parser is adapted from CPython v3.12.12's
[datetime implementation](https://github.com/python/cpython/blob/v3.12.12/Modules/_datetimemodule.c).
Source SHA-256 was
`cd11d1f38e9e637c7404c98ff45e355053b7f7e8251015d58a4c1f17e6d5f8a8`.
The complete license is retained in `crates/rtao-primitives/LICENSE-PSF`
(SHA-256 `3b2f81fe21d181c499c59a256c8e1968455d6689d269aa85373bfb6af41da3bf`),
with attribution and a modification description in `drand.rs`.

Validation used the recorded task root and shared Cargo target:

- `PROPTEST_CASES=10000 cargo test -p rtao-primitives --all-features`:
  41 unit, 1 compact-text, 11 JSON, 8 primitive integration and 4 doc tests pass.
- `cargo test -p rtao-primitives --no-default-features`:
  40 unit, 1 compact-text, 10 JSON, 7 primitive integration and 4 doc tests pass.
- `cargo test -p rtao-catalog --test ratio_denominators`: 1 test passes.
- Strict clippy passes for both crates with all targets/features.
- Rust 1.89 checks pass for both crates with all targets, in both feature configurations.
- Rustfmt checks pass for the new modules and catalog test.

The fetched C source was removed after checking open files/processes, launchd,
configuration references, symlinks and virtual-environment dependencies. The
shared build root remains live for the parent execution task; bounded test
logs remain under its `tmp` directory. No task-specific scratch directory or
live dependency was retained by this helper task.
