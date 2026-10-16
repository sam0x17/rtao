# rtao-runtime

Name-based Bittensor descriptors and typed runtime models. Generated
descriptors identify calls, storage, constants, events and runtime APIs by
name; the active metadata supplies encoded indices and storage hashers.

`RuntimeHandle` shares immutable metadata and its compatibility report.
`Call` composes descriptor arguments into `CallBytes`, whose `RuntimeStamp`
records the runtime identity. Validate that stamp before reusing a composed
call with another runtime.

`calls`, `storage`, `constants`, `events`, `runtime_apis` and `extensions`
contain the generated descriptors. `models` holds handwritten runtime return
types, and `selective` supplies metagraph field selection.

`ShapeHasher` computes exact or projected structural hashes. `verify_surface`
and `verify_surface_with` compare descriptor expectations with metadata and
report breaking, behavioural and additive changes through `SurfaceReport`.

The crate declares no Cargo features. It operates on supplied metadata;
connecting to a node and fetching metadata belong to `rtao-client`.
