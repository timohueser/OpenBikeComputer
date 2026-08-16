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
downloads, and deletes named ordered route collections independently of routes: a trip has its own
name, its own ordered membership, and its own delete semantics, and folding it into route metadata
would put an ordered collection inside the metadata of one of its members. That independent
lifecycle is the whole argument; implementations that omit the feature simply do not advertise the
kind.

`ObjectKind 5` is reserved rather than assigned. The kinds were numbered as the domains were
enumerated and the fifth slot fell to a candidate that did not survive the pass; leaving the hole is
cheaper than renumbering four kinds across three codecs, the storage projection, and every vector, so
the value is permanently unassignable in v2 and a future kind takes the next free number instead of
filling it.

`DeleteObject(Trip)` deletes only the trip. “Delete trip and routes” remains a client-composed
product workflow unless a future requirement justifies an atomic multi-object command.

Routes retain only the validated canonical route payload and catalog metadata. There is no
original GPX/TCX sidecar because no named lossless-source export feature currently requires one;
adding such a product feature would require its own logical-object lifecycle rather than a hidden
storage attachment.

### Weather publishes against the current request only

A bundle publishes if and only if its compare-and-swap revision still matches and its context
matches the current request. A bundle that answers a request the device has since replaced is
rejected at validation as `weather.requestMismatch`, and the current request stays pending until a
bundle that answers it arrives. There is no ranking framework, request-history store, or
weather-specific transaction, and no publication path for a stale bundle.

### The result window is count-bound

The store-global catalog retains the 64 most recent terminal results, including durable Aborted
results. A count-bound window is deterministic without trustworthy wall-clock time. Clients settle
uncertain operations before enough later mutations can evict them. DOS2 (#1354) measures the
storage/RAM cost; changing the count is a protocol and resource decision, not an invisible
implementation tweak.

This deliberately replaces the earlier wall-clock retention wording: there is no minimum time a
result survives, and a client that waits "long enough" has no guarantee at all. The guarantee is
"settle before 64 later terminal results", and the window is store-global across producers — a ride
finalization, a weather transition, a post-boot update state, or a sideload import consumes a slot
exactly as a client's mutation does. A client therefore cannot bound its own uncertainty by counting
only its own requests, which is the consequence first-party clients are written against.

### Update install is explicit but needs no physical confirmation

Uploading a package only publishes `VerifiedReady`. A separate authenticated and authorized
`InstallUpdate` command may automatically arm and reboot after independent package signature,
digest, hardware target, downgrade, resource, and runtime-safety validation. The application
verifies the signature and digest before arming; the bootloader revalidates the package's structure
and CRC framing and enforces trial confirmation and rollback. This removes human confirmation
without collapsing upload and install into one side effect.

Three hardening rules make that removal defensible rather than merely convenient. Version
monotonicity is a **mandatory** device-side admission check with its own rejection detail, because
the peer asking for the change cannot be the authority on what may run. Runtime safety is
enumerated rather than left to implementation taste: no ride being tracked, no unsaved ride data,
and a power threshold. And rotating the release signing key away from the committed test key is a
hard gate before any signed release exists — auto-install plus a published test key would be a
remote-install path, not a convenience.

### USB is a locally trusted link

Attaching the cable establishes the device's local principal, and physical possession of the port is
the authorization boundary for USB. There is no challenge, pairing, or handshake on that link in
v3.0, and every operation the local principal may perform is available over it, including
`InstallUpdate`. BLE is unchanged: it remains bond- and principal-authenticated.

The local principal is one scope rather than one cable: USB attachment, the device's own user
interface, and every device-local producer share it, so a cable client may query and abort
UI-initiated work. Two scopes were considered and rejected, because they would have made a locally
started operation unobservable from the one link a technician has when the display is broken.

The consequence is stated rather than hidden: someone holding the device with a cable can install a
*signed* package. The trust boundary for what runs on the device is the package signature, the
digest, the hardware target, and the anti-rollback check — not the link — and arbitrary code still
requires the release key. Anyone with the device in hand and a cable already has a debug probe's
worth of physical access, so a link-level challenge would protect nothing while making bench work
and field recovery harder. Alternatives considered: a locally entered unlocked mode gating USB
installs, rejected because the mode would have to be enterable on a device whose display or input
may be the thing being repaired; and a USB pairing handshake, rejected because it invents a
credential store for a cable that is already a physical-possession channel.

### Fixed framing, and versions rather than extensions

Every message is a fixed little-endian layout with no in-band way to attach a field the contract
does not define. Evolution is a version decision: appending a field at the tail of a message is a
wire-minor bump a client gates on the minor Capabilities reports, and anything else is a major.
Metadata envelopes stay as the one registry-governed place a domain adds a bounded declared fact,
which is what keeps embedded decoding bounded without turning the protocol into a property bag.

### Publication and result share one durability boundary

No success is observable until one valid catalog commit contains both the logical mutation and its
ObjectResult. A lost notification therefore changes delivery, not truth. Previous generations
remain immutable and leaseable until GC proves they are unreferenced.

The 64-record window is the explicit bound of that guarantee, not an unbounded tombstone promise.
First-party producers never reuse an OperationId. An uncertainty discovered only after eviction is
reconciled from catalog/domain state or surfaced for user resolution, never blindly replayed.

### Multipart work has one durable parent and opaque random child references

`BeginDraft` claims the final manifest intent and compare-and-swap target before any child storage.
Each child has its own OperationId under that parent and seals to a parent-scoped opaque
DraftPartRef. It is not assigned a fake LogicalObjectId, and no GenerationId crosses CardStore's
semantic boundary. Finalizing the parent publishes the manifest and child reachability once. Exactly
one parent may be open at a time, so the 32-part budget belongs to it outright.

A DraftPartRef is 16 random bytes and nothing else. Uniqueness comes from 128 bits inside a live set
the format bounds at 32, and resolution comes from the row the store already writes at seal, which
holds the `(ref, GenerationId)` pair. Validation is therefore a byte-equality lookup among the
parent's sealed rows, which is the only rule that can be sound anyway: a card an attacker can
rewrite makes any self-verifying reference forgeable, and a stored row is not.

Garbage collection still has to resolve a *published* manifest's children after the draft rows are
gone, and it does so from a small durable table rather than from the references themselves. At
finalization the store reserves a generation and writes a bounded resolution generation — `8 + n × 24`
bytes, at most 776 — which the manifest's catalog head names, so a reachability pass is eight
bounded reads and no cryptography at all. There is no key on the card to protect and no primitive in
the storage kernel; the durability argument is the ordinary one about generations and gates. An
unreadable resolution generation makes that manifest's children conservatively reachable, exactly as
unreadable manifest bytes do.

### FAT names are private and 8.3-safe

OBC2 uses one root, alternating checkpoints, a preallocated journal, and sharded generation/work
directories. A low-byte shard plus fixed base-36 high bits represents every `GenerationId` without
long filenames. No physical spelling escapes CardStore.

### Sideloading survives as an import operation, not a second storage path

A card reader stays a delivery path: `/OBC2/IMPORT` is a staging directory, and at mount the device
imports each staged file as a device-local producer — fresh derived OperationId, ordinary blob
transaction, full domain validation, one catalog publication — and deletes the file afterwards.
Routes, standalone maps, and update packages are importable; weather, rides, and trips are not.
Update packages import to `VerifiedReady` only, so the bare-card recovery path survives without
making a copied file an install.

The staged file is never a gated record and is never adopted in place: nothing outside CardStore's
own transaction ever becomes an object. The import identity is *derived* from the store, kind, 8.3
name, and observed length rather than generated, which is what makes a crash between the publication
gate and the file deletion resolve to the retained result instead of a duplicate object. A staged
file that fails validation stays on the card with a terminal Aborted result and is retried only when
its name or length changes, which needs no rename primitive — the FAT adapter has none.

Alternatives considered: adopting a staged file in place as a generation, rejected because it would
put foreign bytes and a foreign name inside the private naming scheme and break the immutability
argument; and dropping sideload entirely in favour of link uploads, rejected because a map is
hundreds of megabytes and a device with a broken radio and a broken cable would have no update path
at all.

### The catalog projection is card-resident

RAM holds a bounded index — head entries, the result-ring index, and the small active/draft/retention
tables, 19,712 bytes at the contract capacities — while the projection envelopes and each manifest
head's resolution reference stay in the checkpoint on card and are re-read on demand. Each
head-index entry carries a `u16` journal-slot reference alongside its fixed fields, which is what
lets compaction find the newest carried head entry without scanning the journal. The alternative, a fully resident projection, is what
the capacities would otherwise have to be cut to fit, and cutting head counts or the result window to
fit an arena would change the contract to meet a budget. DOS2 measures the real figure; it may not
move the projection into RAM to avoid the re-reads.

### The fixed floors are derived, and their alternatives were priced

The 192-byte control-frame minimum is not a round number: it is the largest mandatory v3.0 payload
(a 176-byte maximum catalog entry, and equally a maximum StartUpload descriptor) plus the 16-byte
header. A smaller floor was considered and rejected because it would force either a split catalog
entry or a paging rule that returns zero entries; a larger floor was rejected because BLE's
`ATT_MTU - 3` ceiling would then exclude conservative peers for headroom nothing uses.

Those 176-byte figures are the **schema ceilings**, not what today's registry produces: the largest
producible catalog entry is route's 162 bytes and the largest producible StartUpload is weather's
116. Deriving the floor from the ceiling rather than from the current registry is deliberate — a new
registered field must not silently make the negotiated minimum unusable — and it means the ceiling
cases appear in the vector suite as decode-only rows rather than as traffic any conforming device
emits.

The eight-entry retained-generation table is sized by what can legitimately pin a displaced
generation at once, and with per-kind history gone that sum is seven: four live leases, two
update-rollback entries — the armed handoff's snapshot, plus the one a `complete` handoff still
holds until its cleanup suffix runs — and the single weather domain-retention entry. Eight is that
sum with one entry of margin, which is what makes the "table full" refusal unreachable and lets the
contract retire it. Sixteen was rejected because it sizes the table for a per-kind history this
contract does not keep: the table is checkpoint-resident and every unused entry costs card bytes,
recovery-suffix budget, and lease-clearing work at every mount. There is no
separate handoff retention reason: an armed update's package and rollback snapshot are retained by
the update-rollback reason and by the handoff projection itself.

### Program-page isolation is a volume precondition, not a padding trick

The format's crash argument is that no two gated slots share a media program page. File-relative
padding cannot establish that, because a file offset is not a physical address: a classic LBA-63
partition start or a 4 KiB cluster breaks the mapping no matter how the file is laid out. OBC2
therefore states two normative volume preconditions — a cluster size that is a whole program page
(16 KiB or 32 KiB) and a data region 16,384-aligned to physical LBA 0 — computes both at mount, and
classifies a violation as an unsupported filesystem. Under them, any 16,384-aligned file offset is
physically page-aligned even on a fragmented file, so each slot occupies exactly one page because it
is page-sized and page-aligned. The alternative, measuring alignment at runtime and degrading the
durability argument when it fails, was rejected: a silent rollback window is not something to detect
after the fact, and the device never formats a card, so the check is a statement about how the card
was prepared rather than a repair path. The SD Association formatter and untouched factory formats
satisfy it.

### USB version discovery lives in the descriptors

USB has no out-of-band carrier once framing starts: both endpoint pairs carry length-prefixed
records from the first byte, and the legacy binding's identity read is itself a framed exchange, so
it cannot answer a peer of another major. Enumeration is the only pre-framing conversation on that
link, so the v3 vendor interface reports `bInterfaceProtocol = 3` and `bcdDevice = 0x0300` against
the legacy interface's `0x00` and `0x0010`. Device matching therefore fails cleanly in both
directions before a single record is exchanged. The rejected alternative was a v3 handshake record
at the head of the control stream, which would have to be parsed by exactly the peer that cannot
parse v3 frames.

### The error body reports claim and terminality as two facts

One "claimed and terminal" bit could not describe the common middle case — an error against a live
claimed operation, such as a bad offset mid-upload — and its documented meaning of "clear means no
durable claim" was simply false there. Bit 5 now means a durable claim exists and bit 6 means that
claim is terminal, giving the three answers a client actually needs: reuse the identifier, resume or
query it, or never reuse it. Bit 6 without bit 5 is malformed.

### Store reset is a device-control operation

A store that is corrupt, foreign, or simply unwanted had no wire path to reinitialization, which
left an unusable card recoverable only through the device's own UI — including on devices whose
display is the thing being repaired. `ResetStore` (`0x0406`) adds that path without giving reset
object semantics: it claims no OperationId, retains no result, and echoes the StoreId being
destroyed as its confirmation, with the all-zero echo admitted only where no StoreId is readable. It
returns the new StoreId only once that store's first checkpoint gate is durable. Staged `IMPORT`
files survive, because they are the rider's bytes rather than the store's.

## Simplification round (owner decisions)

Seven features were cut from the frozen suite in one deliberate pass. Each was defensible on its own
and none was load-bearing for a shipping product; together they were paying for machinery — a second
addressing mode, a crypto primitive, a framing mechanism, three parallel resource budgets — that no
first-party client or device path actually needed. The rule applied throughout was KISS with an exit:
every cut number is reserved rather than renumbered, so restoring any of these is an additive
registration rather than a wire break.

**S1 — no requested-revision downloads.** A download resolves the current committed head, full stop.
The old per-ObjectKind history was narrow enough to be a trap: publishing route B destroyed route A's
downloadable past, the catalog never reported what was retained, and a client could not know what it
was allowed to ask for. Removing it deletes the repository-previous retention reason, the
greatest-Revision tiebreak, the lazy reason-clearing garbage-collection step, and the pre-acceptance
durable retention record a lease on a non-head generation used to need. Retention now has exactly
three reasons — a live lease, an update rollback, and one bounded weather domain-retention entry —
and the table falls from 16 entries to 8. Restoring history later would need the retention reason
back, which is additive on card and reuses the burned wire flag. Reserved: StartDownload flag bit 0
and its revision field, `objectNotFound/requestedRevision`, `busy/retainedPrevious`, and
`insufficientSpace/retainedPrevious`.

**S2 — DraftPartRef is random bytes, not a keyed codec.** The Feistel/HMAC construction put a
cryptographic primitive, a 32-byte key on removable media, and a security review inside the storage
kernel to buy a property a table buys for 776 bytes. The reference is now 16 random bytes resolved by
byte-equality against the row the store already writes at seal, and published manifests carry a
bounded resolution generation that garbage collection reads instead of decoding refs. The forgery
argument gets simpler rather than weaker: there is nothing to forge that resolves, because resolution
is a lookup.

**S3 — no weather supersession.** One publication rule, one committed outcome. A bundle answers the
current request or it is rejected. The superseded path existed to salvage a bundle that arrived after
its request moved, which the client can simply re-request with the context it now reads. Reserved:
ObjectResult outcome `1` and `weather.supersededNotUseful`.

**S4 — no extension blocks.** Messages are fixed layouts. Protocol evolution is a wire-minor bump for
a tail-appended field and a major for anything else, which is the rule the contract already relied on
for everything the extension block never carried — no v3.0 extension was defined, none participated
in intent, and the 192-byte floor deliberately left it no headroom. Metadata envelopes are untouched:
they are the registry-governed mechanism domains actually use.

**S5 — one draft parent.** Two parents bought concurrent multipart uploads that one
heavy-transfer coordinator cannot run anyway, at the cost of a global-versus-declared part
reservation rule spanning three documents. One parent owns the whole 32-part budget, a second
BeginDraft is `busy/draftParents`, and the checkpoint's draft-parent region halves.

**S6 — the patch schema is pinned.** The subject entry advertised a per-kind patch-schema version as
if it were negotiable. It is a constant of the registry: `128` when a kind advertises set-metadata
and zero when it does not, with any other value rejected. The byte stays in the frozen layout.

**S7 — resume is one bit.** `forbid`/`prefer`/`require` had one outcome the other two did not: a
`require` against absent work was the only way an upload could be refused for a resume reason, and a
client that hit it had to retry with a different preference. The byte now means resume-permitted or
restart-at-zero, every combination is accepted, and the acceptance flags already tell the client
which happened. Reserved: `objectNotFound/resumableWork` and resume values above `1`.

## Rejected alternatives

- Reusing wire major 2: an existing peer could pass version discovery and misdecode traffic.
- Preserving v1 card/wire compatibility: it would require dual authority and retain superseded
  staging/promotion behavior.
- Time-based result retention: it depends on trustworthy device time or can block new mutations
  while unexpired slots occupy a bounded ledger.
- A separate retention window for device-local producers: two windows would double the durable
  ledger to hide a consequence clients must handle anyway.
- Adopting a sideloaded file in place instead of copying it through a blob transaction: it would
  make a foreign name and foreign bytes part of the private storage model.
- A USB pairing handshake or an unlocked-mode gate on USB installs: neither adds a boundary that
  physical possession of the port has not already crossed.
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
