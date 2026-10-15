# Getting started

Use a release tag as the git dependency and match its Bittensor core patch:

```toml
[dependencies]
rtao = { git = "https://github.com/sam0x17/rtao", tag = "rtao-v0.1.0", default-features = false, features = ["client"] }
```

Copy the exact `[patch.crates-io]` entry from that release's notes into the
workspace root. The patch must track the release's core pin. Check it with:

```sh
rtao doctor --manifest-path Cargo.toml
```

The first CLI release contains `doctor`, `--help` and `--version`. Additional
CLI commands arrive in later milestones. Consult the [API index](api-index.md)
and the source tag's compatibility record when using a library feature.
