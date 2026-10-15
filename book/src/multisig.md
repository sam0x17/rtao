# Multisig and external signing

A multisig account derives from the sorted public signatory keys and its
threshold. Each approval is signed by one member. The multisig account itself
has no private signing key. Use `Executor::multisig` to inspect or compose an
approval, and retain the opening timepoint when approving an existing operation.
A hash-only approval cannot execute the inner call at the threshold; execution
requires the full call.

For a typed semantic operation, use `MultisigAdapter<I>` for thresholds of two
or more and `MultisigThreshold1Adapter<I>` for immediate one-of-N dispatch.
Both own the inner intent, retain its policy/preflight/shield requirements and
sign with the member's coldkey. The effective semantic origin is the multisig
account, with semantic Sudo and proxy layers inside the multisig wrapper.

Call `adapter.review(executor, keys, options)` to obtain a plan and an opaque
`MultisigContinuation`. Pass that continuation to a later adapter constructor
to reuse the exact reviewed bytes, including randomized calls. It binds the
semantic arguments, signer set, effective origin and runtime; a different
intent cannot inherit its policy label. For a pending operation, also retain
and require its opening timepoint with `with_opening_timepoint`. An absent or
replaced opening refuses. A continuation proves where the bytes came from;
each approval still performs current policy and preflight checks.

An external signer signs the member's prepared approval transaction. Compose
proxy, Sudo and multisig wrappers before preparing it, then call
`Executor::prepare_call(call, member_address, &PrepareOptions)`. Preparation uses
the public account and fetches a fresh nonce without reserving a cached nonce.
Export with `serde_json::to_vec(&unsigned)` or `unsigned.to_json()`.

The exported object has Python's `UnsignedExtrinsic.to_dict()` fields, including
the SCALE call, signer account and algorithm, runtime versions, era, payload,
JSON signer view and both signed-extension byte parts. Sign the prepared payload
as provided; it is already hashed when its original length exceeds 256 bytes.
For a metadata-verifying signer, supply its independently computed metadata hash
and matching display decimals and token symbol in `PrepareOptions`.

Import a returned file with `Client::import_unsigned_json(data, &SubmitOptions)`.
Import authenticates the runtime, genesis and mortal era anchor, then compares
the file's payload, JSON view and signed parts against the core encoder. Missing
fields, unsupported algorithms, native integer overflow and altered signing
views are rejected. `UntrustedUnsignedExtrinsic` also supports validation with
independently authenticated runtime and chain inputs for an offline application.

Submit the resulting signature with
`Executor::submit_signature(&unsigned, signature, &ExecuteOptions)`. An active
policy must allow raw calls. The executor checks the current policy even for an
imported file; JSON carries no policy authorization. Submission verifies the
signature against the member account, refuses expired mortal eras and refuses
either a spec or transaction version change. After an upgrade or expiry,
prepare and sign again.
