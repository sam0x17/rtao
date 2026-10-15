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
pinned SCALE layouts, canonical decoding, unchanged call bytes across five
SS58 prefixes, preserved display/JSON spelling, exact warnings and policy,
and malformed constructor inputs. The hash reference is
`attocash/commons@5ae16299214ed7fd44bc33c3ee6ee437cde393d9`,
`commons-core/src/commonTest/kotlin/cash/atto/commons/AttoHashTest.kt:9-18`.
The fake-transport fixtures also require no additional reads or submissions.

The CLI enables all intent interfaces. Its existing generated availability
checks continue to advertise only implemented operations, so SDK source ports
become available through `tx` without another per-group manifest change.
Pending operations remain absent from the command tree.

Hosted generation of three existing pass-through maps, four pending-body
removals and availability/report updates is required before combined checks.
Only source review, rustfmt and whitespace checks ran locally; no tests,
builds or scratch directories were created.
