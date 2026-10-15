# M2 transaction intents

All nine M2 intent behaviors are implemented locally: transfer/transfer-all, four serving operations, and set/commit/reveal weights. Hosted oracle vectors, localnet scenarios and milestone exits remain pending.

Transfer resolves exact amounts into TAO before encoding, chooses keep-alive/allow-death/all exactly as Python does, and reports bounded or unbounded spend. Summaries and sweep warnings preserve Python's wording. Validated SS58 inputs retain the caller's network prefix in JSON and summaries, while encoding the decoded account bytes. Transfer-all delegates its build to the generated pass-through function. Native u64 limits and TAO/alpha unit checks follow Q25/D-035.

Serving emits typed calls for IPv4, IPv6 and scoped IPv6 addresses. Certificate decoding preserves Python's lowercase `0x` prefix handling, six ASCII whitespace characters, Unicode precedence and error positions. D-017 rejects certificates over 65 decoded bytes after complete hex validation: the pinned chain permits one algorithm byte and at most 64 public-key bytes, silently discarding oversized certificates. IP errors precede hex errors, which precede length errors. Endpoint strings remain unchanged in summaries; construction validation remains deferred to build as in Python. Ten focused serving tests pass on current Rust and Rust 1.89, including exact call bytes, reset placeholders, the 65/66-byte boundary and error ordering.

The root owner implemented weight preflight, rate and Null-permit checks, mechanism-index storage, conforming, plaintext/commit selection, clock injection, corrected epoch prediction and legacy reveal. Their tests and detailed scheduling evidence remain in the existing weight execution records. The interface owner added mapping/list carriers and Python constructor normalization.

`crates/rtao-intents/tests/hosted_oracle.rs` uses the unified native adapter in `xtask/src/oracle/replay.rs` to replay the designated CI lane's intent/read vectors and each immutable sample seed. It compares constructor JSON, call bytes through the core encoder, build extras, preflight fields, spend, exact descriptor hit sets, fee rao and complete executor plans. Only the oracle's explicit randomized commitment/reveal-round masks are admitted. It closes injected clients and rejects any submission request. Two native input tests construct and encode every reviewed intent sample and dispatch every reviewed read sample at both blocks. These checks prepare the adapter; Python differential results still require the hosted output.

The hosted test requires `RTAO_ORACLE_DIR` and is deliberately ignored in ordinary local runs:

```sh
RTAO_ORACLE_DIR=/path/to/verified/artifacts cargo test -p rtao-intents \
  --features all --test hosted_oracle hosted_m2_intent_and_read_oracle_replay -- --ignored
```

No Python or local oracle containers were executed. Current Rust and Rust 1.89 aggregate suites pass 38 intent library tests and two native oracle-input tests, with strict all-target Clippy for primitives/intents/xtask, four generator tests and no-feature compilation. The seven policy-equivalence tests pass and the localnet e2e target compiles with its Docker feature. The separate hosted test remains pending actual immutable vectors; full gates must pass before any push or milestone tag.
