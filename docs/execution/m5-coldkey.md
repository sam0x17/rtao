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
at `18679e34800d237cd20c43566b63d17f473032fd` succeeded and was adopted
unchanged. Review checked thirteen exact regular-file members, source identity,
all 23 dependency inputs, unchanged role package sets and counts, four pending
removals and three pass-through additions. Other generated interfaces and
matrix families are unchanged. The generated ZIP is 54,283 bytes, SHA-256
`82c105e55704648e21bb2a5f8b0728154ebbde822aca4dc114d60aaefbc27a6f`;
the report ZIP is 40,189 bytes, SHA-256
`b9ec45e7d1c8ab0b864fcabb28d6e5a6ebe0b9ff5a22f848fee86bf2c70042af`.
The bounded private proof is
`.local/recovery/2026-10-14/m5-coldkey-adoption.json`.

Runtime validation remains empty. Combined compilation, fixture execution and
milestone acceptance remain pending. Only source review, rustfmt and whitespace
checks ran locally; no tests, builds or scratch directories were created.

Source review found unused intent parameters in the generated zero-argument
builds, which would fail strict Clippy. The generator now names an unused
binding `_intent`. Follow-up run
[38696690648](https://github.com/sam0x17/rtao/actions/runs/38696690648)
at `7c9c8cac5394099f48a420effc454fbc0afe071f` succeeded. Its output was
adopted unchanged after verifying exactly those two generated parameter-name
changes, unchanged other interfaces/matrix, all thirteen archive members,
23 dependency hashes and unchanged role inventories. The private proof is
`.local/recovery/2026-10-14/m5-fieldless-adoption.json`. This generator result
does not establish successful ordinary fixture execution.
