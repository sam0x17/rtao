# Unicode correction in submitted protocol formatting

The independent audit found a parity defect in PR U1
[#3246](https://github.com/RaoFoundation/subtensor/pull/3246), inherited by U2
[#3247](https://github.com/RaoFoundation/subtensor/pull/3247). The submitted
`unicode-general-category 1.1.0` uses Unicode 16, while the pinned oracle,
CPython 3.12.11, uses Unicode 15. A nested diagnostic containing U+1C89 emits
the character in the submitted public `result_message()` API. CPython's
`repr` requires `\u1c89`. Earlier passing protocol tests did not cover
characters newly assigned in Unicode 16.

## Authenticated source and independent reference

CPython tag `v3.12.11` resolves to tag object
`228e453fc225431bd149d390588e7ed2bc24d0fb` and commit
`55fee9cf216abe4ec0d1139f94b1930fbd0c7644`.
Its `Tools/unicode/makeunicodedata.py` names Unicode `15.0.0`; printable
characters are ASCII space and categories outside `C` and `Z`.
`Objects/unicodeobject.c` escapes other codepoints with lowercase `x`/`u`
or uppercase `U`, using two, four or eight hexadecimal digits.

Official versioned UnicodeData 15 has SHA256
`806e9aed65037197f1ec85e12be6e8cd870fc5608b4de0fffd990f689f376a73`.
The pinned CPython generated `Objects/unicodetype_db.h` has SHA256
`ab60f0ca6db86dc14bb19ca5bcce6b0274282172500dd1c3ef61030e4b080cc8`.
The retained provenance manifest records every source URL, size and hash.

A small C evaluator reads that authenticated generated CPython table,
using the printable and decimal flag masks and generated whitespace
function. It evaluates all 1,114,112 codepoints. It neither embeds a Python
runtime nor executes a Python interpreter. A separate Ruby generator parses
official UnicodeData, expands First/Last ranges, and independently compares
all three properties for every codepoint. There are zero differences.
The CPython source generator was read as text only.

## Native proof and correction

The original Rust library is built from the exact submitted U1 head,
`9e926d1f2f4943f11a23ef13ff517e7857e9cc27`, using its distinct workspace
wrapper and Rust 1.89.0. The proof links the emitted hashed core artifact
`libbittensor_core-3c70e22f9ca37e7f.rlib`, rather than an unqualified library
path in the shared build directory. Its Cargo artifact record identifies
the U1 manifest and reports an actual build.

The proof calls the submitted public `DispatchErrorInfo::result_message()`
with nested `Value::Tuple([Value::Str(...)])` diagnostics. U+1C89,
U+10D40, U+10D41, U+11BF1 and U+116D1 reproduce the mismatch. A scratch
copy of the submitted formatter, corrected to use generated Unicode 15
printable ranges, produces the required escaped string for each.

| Native comparison | Result |
| --- | --- |
| Generated printable, decimal and whitespace properties against native CPython rows | All 1,112,064 Rust scalars match |
| Printable table intended for shipping against native CPython rows | All Rust scalars match |
| Submitted Unicode 16 printable rule against CPython 3.12 | 5,812 differing scalars |
| Submitted Unicode 16 decimal rule against CPython 3.12 | 80 extra accepted digits; no existing value changes |
| Rust whitespace plus U+001C-U+001F against CPython `str.isspace` | Zero differences |
| Original and corrected public formatter across quotes, controls, escape boundaries and Unicode 15 positive U+1E4F1 | 16 cases pass in both |
| Public formatter on the five Unicode 16 witnesses | Original differs; corrected matches |

The shipping table contains 711 printable ranges, uses no dependencies,
and is independent of the compiler's Unicode version. The complete
Unicode-DFS-2016 copyright and permission notice is embedded in its source,
using the first license section from immutable ICU 72 commit
`ff3514f257ea10afe7e710e9f946f68d256704b1`. The table is generated from UCD
data; CPython's table serves as a separate verification reference.

An independent reviewer rechecked the source hashes, generator behavior,
range expansion, binary-search bounds, quote selection and escape widths.
The review identified the full Unicode notice as a shipping requirement;
the generated shipping table now includes it.

## Root consumers and dependency boundary

The same Unicode 16 mismatch affects root primitives and RPC string
formatting, integer/float/ratio parsing, btauth numeric header parsing,
configuration cache keys, custom pool-error conversion, hyperparameter
proportions and timelock durations. Existing Unicode 15 digits must remain
accepted, including U+1E4F1. The auth finding concerns accepted spellings and
interoperability; this audit found no signature or replay bypass.

`unicode-general-category 0.6.0` declares Unicode 15 and has no dependencies.
Its crate archive matches the official registry SHA256
`2281c8c1d221438e373249e065ca4989c4c36952c211ff21a0ee91c44a3869e7`.
Its built native API, using the current production printable and decimal-run
algorithms, matches CPython 3.12 across all Rust scalars, including exactly
680 decimal digits. Root can pin this exact version for existing consumers.
The staged core extension instead uses the authenticated generated table,
preserving its required upstream dependency boundary.

Whitespace behavior remains contextual: Python's direct numeric parsers
have narrower ASCII rules than `str.isspace`; hyperparameter and duration
helpers deliberately strip first. No blanket whitespace change is proposed.
The RPC format-character sanitizer is unaffected by the Unicode version
change: both versions contain the same 170 `Cf` characters.

## Reviewable audit output

Prototype files are under the recorded task root at
`upstream-quality/unicode-audit/`:

- `python_printable.rs`, `generate_unicode15.rb`, `provenance.json` and source licenses.
- `upstream-unicode15.patch`: submitted U1 formatter, generated table,
  dependency/lock removal and a focused 21-case public regression.
- `root-staged-unicode15.patch`: root staged formatter/table, removal of
  its unsupported dependency, and the same focused regression.
- `patch-manifest.json`: original and corrected file hashes.
- `unicode_probe.rs`, `category06_probe.rs`, `cpython_unicode_oracle.c`
  and `cpython_unicode15.bin`: reproducible native/source proof.

Both patches pass `git apply --check` against their recorded originals.
The root patch intentionally leaves the workspace version and lock update
to root integration. Neither published branch nor root production source was
modified during the audit. Root subsequently reviewed the patch and authorized
the publication below. U1/U2 stacked ancestry remains intact.

The final proofs are recorded in task-root `tmp/` as
`quality-unicode-public-api-proof-final.log`,
`quality-unicode-category06-proof.log` and
`quality-unicode-generator.log`. The native proof passes on Rust 1.89.0;
the audit prototype did not yet establish integrated branch validation.
No hosted Python execution or runtime oracle replay is claimed.

The bounded source/proof archive is `tmp/quality-unicode-proofs.tar.gz`,
502,308 bytes, SHA256
`2356d197fb4f82be7c2a27194463d94f0fd5142e72736d7295e2106f8c9fe75d`.
After checking processes, open files, launchd, configuration, symlinks and
virtual-environment references, the three compiled probe executables and
the separate 0.6.0 build directory were removed, freeing 12,142,842 bytes.
`tmp/quality-unicode-cleanup.json` records the exact paths and zero live
dependencies. Authenticated source, generated tables, prototype patches and
reference rows remain live for root integration review; shared build and
original wrappers remain owned by root. The later publication removed its two
finished worktrees and disposable targets after a separate audit.

## Published correction

The authorized ordinary follow-up pushes completed on 2026-10-14:

| PR | Published corrected HEAD | Portable tests per compiler |
| --- | --- | --- |
| [U1 #3246](https://github.com/RaoFoundation/subtensor/pull/3246) | `77480991b2b3b39ec828d2da9d7c6ecf31113a81` | 27 |
| [U2 #3247](https://github.com/RaoFoundation/subtensor/pull/3247) | `849c192e9702fa245a6f16941359290786554d85` | 31 |

Both protected pre-push hooks validated the exact clean committed HEAD with
Rust 1.89.0 and 1.99.0. Each compiler passed the original portable suites plus
one new public formatter test containing 21 cases, 71 all-feature library tests
with the existing physical-OpenCL-GPU ignore, and all-feature/all-target binding
compilation. Pinned 1.89.0 strict core/binding Clippy and both portable wasm
checks passed. Binding runtime tests were not run; the binding defines no Rust
unit tests. Current strict Clippy is not claimed because unchanged base lints
remain. All correction Cargo commands set `PYO3_NO_PYTHON=1`.

The full Unicode notice ships in the generated table, and the upstream core
license expression is `Apache-2.0 AND Unicode-DFS-2016`. Offline scoped Cargo
checks canonically removed exactly the dependency edge and 1.1.0 package from
each lockfile. Both final locks are byte-identical to v478. Source guards matched
the reviewed formatter/table/test hashes and rejected changes outside the five
correction paths. All original published ancestry is preserved.

GitHub API and fork branch SHAs agree. Existing PR bodies were updated and
fetched back for exact comparison; labels and review requests remain empty.
Each new SHA has ten hosted workflows ending in `startup_failure`, all with
zero jobs and zero check-runs. Vercel requires authorization. Hosted Python
execution and live integration remain pending.

The [publication record](quality-unicode-publication.md) links the actual logs,
artifact/source hashes, body snapshots, remote evidence and cleanup results.
Incremental corrective patches are retained separately from the original U1/U2
patches. No local Python, Docker or host-setting change was used.
