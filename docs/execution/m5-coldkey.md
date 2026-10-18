# M5-02 coldkey migration intent source

The four coldkey intents preserve the pinned Python summaries, warnings,
default effects and no-spend policy. Only `SwapColdkeyAnnounced` declares a
global subnet effect. Timing, self-swap and announcement-state checks remain
chain dispatch decisions. Source fixtures perform no wallet operation or real
submission.

`coldkey.py` at parity commit `850f2755bb05a3851f948489f0ab27b0645cebce`
was authenticated against GitHub and the vendored bytes: Git blob
`2afbd304453223a3f3c4719cdcd13c85d41f98d3`, SHA-256
`82e24717ed5431e501c90a9b8c3537a968d55016577edaeac509eebe73569315`.
Announcement hashes use the existing core BLAKE2b-256 implementation on the
decoded 32-byte public key. They do not hash the SS58 spelling or truncate a
512-bit digest.

Five fixture sources check an independent Bouncy Castle hash vector, exact
pinned SCALE layouts, canonical decoding, unchanged call bytes across four
SS58 prefixes, preserved display/JSON spelling, exact warnings and policy,
and malformed constructor inputs. The core refuses unknown SS58 format 1000;
the prefix fixture now covers that refusal separately. The hash reference is
`attocash/commons@5ae16299214ed7fd44bc33c3ee6ee437cde393d9`,
`commons-core/src/commonTest/kotlin/cash/atto/commons/AttoHashTest.kt:9-18`.
The fake-transport fixtures also require no additional reads or submissions.

The CLI enables all intent interfaces. Its existing generated availability
checks continue to advertise only implemented operations, so SDK source ports
become available through `tx` without another per-group manifest change.
Pending operations remain absent from the command tree.

Hosted generation
[38696143491](https://github.com/sam0x17/rtao/actions/runs/38696143491)
at `b458d291f2b4c6bf14600c4747ce3d69d2736718` succeeded and was adopted
unchanged. Review checked thirteen exact regular-file members, source identity,
all 23 dependency inputs, unchanged role package sets and counts, four pending
removals and three pass-through additions. Other generated interfaces and
matrix families are unchanged. The generated ZIP is 54,283 bytes, SHA-256
`82c105e55704648e21bb2a5f8b0728154ebbde822aca4dc114d60aaefbc27a6f`;
the report ZIP is 40,189 bytes, SHA-256
`b9ec45e7d1c8ab0b864fcabb28d6e5a6ebe0b9ff5a22f848fee86bf2c70042af`.
The bounded adoption proof is not retained in the repository.

Runtime validation remains empty. Combined compilation, fixture execution and
milestone acceptance remain pending. Only source review, rustfmt and whitespace
checks ran locally; no tests, builds or scratch directories were created.

Source review found unused intent parameters in the generated zero-argument
builds, which would fail strict Clippy. The generator now names an unused
binding `_intent`. Follow-up run
[38696690648](https://github.com/sam0x17/rtao/actions/runs/38696690648)
at `6d43f697a460f2d3cd3670c3795b7532c2678d5b` succeeded. Its output was
adopted unchanged after verifying exactly those two generated parameter-name
changes, unchanged other interfaces/matrix, all thirteen archive members,
23 dependency hashes and unchanged role inventories. The bounded adoption proof is not retained in the repository. This generator result
does not establish successful ordinary fixture execution.
