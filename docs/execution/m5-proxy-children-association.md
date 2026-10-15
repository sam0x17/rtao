# M5-02 proxy, children and association source

The candidate batch adds six proxy, five children/take and two association
intents. Generated integration and ordinary hosted tests remain pending;
these thirteen are not yet included in the committed count of 36/83.

The Python sources at `850f2755bb05a3851f948489f0ab27b0645cebce` were
authenticated against GitHub and the vendored bytes:

| File | Git blob | SHA-256 |
|---|---|---|
| `intents/proxy.py` | `e19113d503ef0b2f488af38bec57cadb934e1f56` | `c142c20b55544ac1d093aa5d35b3960050dc283e1add20ff6dbedc83e65aea0e` |
| `intents/children.py` | `0dd6874119b12211ee566b5a53bab05e9128c8e9` | `f4f6cf51f8a6cfe4a531f397a7c9b1f3187ce9e3925932b5d9ce3bf1c05f914e` |
| `intents/association.py` | `289fc369c7f702231460852af4b3a7f7ae348041` | `9a051af7f1e85fbc2f613a838e863888127843833b61c54e29452d7c846898c2` |

Announced proxy calls build their inner intent against the caller's original
public wallet and pinned view. They retain the semantic call without adding
executor wrappers or inner extras, and preserve the outer default policy and
preflight behavior. D-020 records this kept behavior. D-046's shared scanner
now guards both proxy and crowdloan constructors before serde conversion,
recursive cloning or nested construction. Existing crowdloan fixtures exercise
literal node/byte limits and retained-copy work limits; proxy fixtures cover
its constructor entry point, private carriers, depth and accepted dispatch.

Children and delegate takes preserve Python's distinction between raw integers
and fractional inputs, including its floating-point rounding at u64 boundaries.
Share normalization precedes child-address encoding. `SetTake` reads the current
delegate take and chooses the increasing or decreasing call, refusing equality.
Chain restrictions on child count, duplicates, root subnet and configured take
bounds remain dispatch decisions.

Constructor normalization also follows Python's two-item iterable unpacking
for JSON strings and objects. Child accounts accept native SS58 and lowercase
`0x` hex spellings at build time. The existing JSON carrier cannot retain an
object's original insertion order or represent Python bytes/bytearray; JSON
integer arrays are native lists and remain invalid accounts. Diagnostics for
malformed non-string child values do not reproduce every Python conversion
error or its precedence. These limits are not evidence of full Python-object
or malformed-input diagnostic parity.

Hotkey association retains public wallet defaults and SS58 spellings. EVM
association uses the checked runtime descriptor with dynamic signature bytes
to preserve hex syntax, H160 and fixed-length validation order. Its hex parser
follows CPython v3.12.11 `_PyBytes_FromHex`, blob
`dae84127a7df4b7aa2bb328285b733a9896bc01d`, SHA-256
`ba826eab3af4004103dd8ad99dff4ed6d23c35c516ec9fd773f77eddc4942c2d`.
It does not verify the ownership proof locally.

Fixtures check independent SCALE layouts, decoded argument values, enum and
integer boundaries, input normalization, prefixes, exact warnings, policy and
public-origin handling. They use in-memory fake transports. Source review
removed premature staking-only helper cfgs, which would have introduced unused
functions before the staking consumers existed.

No local build, test, Python process, scratch root, node, wallet operation or
upstream filing was used for this batch. Formatting and source checks cannot
establish successful fixture execution or milestone acceptance.
