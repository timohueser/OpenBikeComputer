# DOS1 — Device Object System v2 contract decisions

- Status: accepted
- Date: 2026-08-16
- Scope: #1256, #1358
- Normative suite index: [`../../specs/Device_Object_System_v2.md`](../../specs/Device_Object_System_v2.md)

## Context

The previous transfer architecture made filenames, 16-bit IDs, temporary files, promotion schemes,
and per-domain runners observable across storage, transports, clients, and UI. Preserving those
shapes would keep the same ownership problems behind new names. DOS v2 is therefore a coordinated
wire/on-card break based on immutable generations and one catalog publication boundary.

## Decisions

### Wire major 3 names DOS v2

The shipped descriptor protocol already reports major 2. The new control and stream frames use
major 3 so an old peer cannot mistake them for compatible traffic. “Device Object System v2” names
the architectural/on-card generation. Product version numbers may be reset before public launch;
that future coordinated edit must recut every vector and peer together.

### Identity types are not aliases

Store, logical object, revision, physical generation, durable operation, stream session, and
control request identities are separate types. The compiler/API boundary must prevent accidental
substitution even where two representations have the same width.

### One small public contract, concrete domain policy

Common framing and transactions move bounded bytes and publish results. Concrete repositories own
their validators and metadata. There is no repository trait containing every domain method and no
transport façade forwarding domain operations.

Trip remains an optional separate kind because the product already creates, replaces, lists,
downloads, and deletes named ordered route collections independently of routes. This is the
independent lifecycle required by #1356; implementations that omit it do not advertise the kind.

`DeleteObject(Trip)` deletes only the trip. “Delete trip and routes” remains a client-composed
product workflow unless a future requirement justifies an atomic multi-object command.

Routes retain only the validated canonical route payload and catalog metadata. There is no
original GPX/TCX sidecar because no named lossless-source export feature currently requires one;
adding such a product feature would require its own logical-object lifecycle rather than a hidden
storage attachment.

### Weather supersession is a narrow repository decision

A bundle for an older request may publish only if its compare-and-swap revision still matches, it
validates against the current request context, and either there is no current head or its issued
time is strictly newer than the current head. It returns a distinct committed outcome and does not
satisfy the newer request. This deliberately avoids a ranking framework, request-history store, or
weather-specific transaction.

### The result window is count-bound

The store-global catalog retains the 64 most recent terminal results, including durable Aborted
results. A count-bound window is deterministic without trustworthy wall-clock time. Clients settle
uncertain operations before enough later mutations can evict them. DOS2 (#1354) measures the
storage/RAM cost; changing the count is a protocol and resource decision, not an invisible
implementation tweak.

### Update install is explicit but needs no physical confirmation

Uploading a package only publishes `VerifiedReady`. A separate authenticated and authorized
`InstallUpdate` command may automatically arm and reboot after independent package signature,
digest, hardware target, downgrade, resource, and runtime-safety validation. The bootloader still
revalidates and enforces trial confirmation/rollback. This removes human confirmation without
collapsing upload and install into one side effect.

Both BLE and USB must establish an application principal; cable possession alone is not
authorization outside an explicit locally entered developer/unlocked mode.

### Fixed framing with bounded extensions

Common message fields use fixed little-endian layouts. Only messages that need evolution carry a
small sorted extension block with an explicit mandatory bit. This keeps embedded decoding bounded
and makes unknown-field behavior precise without turning the protocol into a generic property bag.

### Publication and result share one durability boundary

No success is observable until one valid catalog commit contains both the logical mutation and its
OperationResult. A lost notification therefore changes delivery, not truth. Previous generations
remain immutable and leaseable until GC proves they are unreferenced.

The 64-record window is the explicit bound of that guarantee, not an unbounded tombstone promise.
First-party producers never reuse an OperationId. An uncertainty discovered only after eviction is
reconciled from catalog/domain state or surfaced for user resolution, never blindly replayed.

### Multipart work has a durable parent and authenticated opaque child references

`BeginDraft` claims the final manifest intent and compare-and-swap target before any child storage.
Each child has its own OperationId under that parent and seals to a parent-scoped opaque
DraftPartRef. It is not assigned a fake LogicalObjectId, and no GenerationId crosses CardStore's
semantic boundary. Finalizing the parent publishes the manifest and child reachability once.

The private storage codec uses a keyed reversible mapping plus a 64-bit context check rather than
a public physical ID or an unbounded reference table. The reference is not an authorization
capability: normal principal and operation authorization is still mandatory. Guessing or
substituting a reference succeeds with probability at most 2^-64 under the non-adversarial storage
integrity model; possession of an offline card exposes the key and is outside that model because it
already exposes the payload bytes. DOS2 must measure the bounded HMAC cost for the 32-part maximum
and obtain a focused security review before retaining this codec. If that review rejects its cost
or construction, a replacement must preserve the same 16-byte wire type, parent binding, bounded
lookup, and no-GenerationId exposure and must recut the storage vectors.

### FAT names are private and 8.3-safe

OBC2 uses one root, alternating checkpoints, a preallocated journal, and sharded generation/work
directories. A low-byte shard plus fixed base-36 high bits represents every `GenerationId` without
long filenames. No physical spelling escapes CardStore.

## Rejected alternatives

- Reusing wire major 2: an existing peer could pass version discovery and misdecode traffic.
- Preserving v1 card/wire compatibility: it would require dual authority and retain superseded
  staging/promotion behavior.
- Rejecting every superseded weather response: it discards still-useful newer data and forces work
  to repeat.
- Time-based result retention: it depends on trustworthy device time or can block new mutations
  while unexpired slots occupy a bounded ledger.
- Physical update confirmation: not required once the command principal and signed package are
  independently trusted; trial boot and rollback remain the safety boundary.
- A single union-of-domains repository or client capability bag: it would recreate the façade that
  the epic is replacing.
- Arbitrary metadata TLVs: they weaken validation and allow clients to invent repository policy.

## Consequences

- The normative suite is split into a small system index, wire contract, storage format, bounded
  domain registries, and one vector/transcript inventory. No file owns every layer.
- Rust, Swift, and TypeScript require new independent codecs and strong identity wrappers.
- The existing wire-v2 spec remains legacy evidence during development and is deleted from shipping
  authority in DOS12.
- DOS2 (#1354) must prove catalog/result atomicity and the private naming scheme under every media cut.
- DOS3 can implement one state machine because transport differences are isolated to adapters.
- Every domain migration updates only its affected registry/contract and the shared vectors before
  adding a schema or outcome.
