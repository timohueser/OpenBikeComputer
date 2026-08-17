//! The route repository: the first domain to own its own admission, validation and projection.
//!
//! `Device_Object_Registries_v2.md` fixes everything this file implements, and it is worth reading
//! the three rows together because they are what make a route repository non-trivial:
//!
//! - §4.1 — a route **Put** declares exactly one field: retention, `0..=5`.
//! - §4.3 — a route's **catalog projection** requires a display name *and* a retention, and adds two
//!   optional facts (selected, trusted creation time) a device emits "only when the device holds the
//!   fact".
//! - §4.2 — a route **patch** may carry retention, selected, or a display name, and "every present
//!   field is applied in one catalog commit".
//!
//! The name is the interesting one. A Put carries no name, and the projection requires one, so the
//! name can only come from one place: the payload. §4.3 says as much — catalog projections "contain
//! validator-derived bounded facts" — and `OBCR_Spec.md` §1 puts a 48-byte UTF-8 name at header
//! offset 64 with its used length at offset 6, which is exactly the 1–48 bytes the projection field
//! accepts. So validating the payload and deriving the projection are one pass over one 128-byte
//! header, and neither is possible without the other.
//!
//! ## What this validates, and what it deliberately leaves to DOS5
//!
//! It validates the **frame** of an OBCR file: magic, the one accepted version, the reserved bytes,
//! a name that is present and clean, at least one chunk, and every section offset the header
//! declares landing inside the sealed length. That is the check the registry's one route detail —
//! `invalidRouteFormat` — exists for, and it is enough that a head can never name bytes a reader
//! will fault on when it tries to open them.
//!
//! It does not walk the chunk index, cross-check per-chunk byte ranges, verify seam-sharing vertices
//! or re-derive the stats. Those are payload-interior rules, they need a reader over the whole file,
//! and DOS5 owns them along with the fuzzing that goes with them. The seam they arrive through
//! already exists: [`SealedBytes`] reads the whole generation, not just its head.

use obc_formats::obcr;
use obc_link::engine::FailureCause;
use obc_link::error::detail;
use obc_link::frame::Opcode;
use obc_link::ids::LogicalObjectId;
use obc_link::metadata::{
    text_is_clean, MetadataEnvelope, MetadataWriter, Schema, SchemaClass, MAX_CATALOG_ENVELOPE,
    MAX_REGISTERED_MUTATION_ENVELOPE,
};
use obc_link::registry::{retention, subject_flags, ObjectKind};
use obc_link::upload::Target;

use crate::obc2::limits::{MAX_GENERATION_LEN, MAX_ROUTE_HEADS};
use crate::obc2::transaction::{CatalogProjection, KernelMedia, SealedBytes, Validation};

use super::{not_found, revision_conflict, Capability, HeadView, EMPTY_ENVELOPE};

/// `Device_Object_Registries_v2.md` §6: the one semantic detail the route namespace registers.
pub const INVALID_ROUTE_FORMAT: u16 = 1;

/// The subject operation flags §1's matrix permits a route: put, get, delete, set-metadata.
pub const OPERATIONS: u16 =
    subject_flags::PUT | subject_flags::GET | subject_flags::DELETE | subject_flags::SET_METADATA;

/// The base tag a full tag names: §2.2 makes the critical bit part of the *encoding*, not of the
/// field's identity, so a writer pushes the full tag and a reader looks the field up by its base.
const fn base(tag: u16) -> u16 {
    tag & !obc_link::metadata::CRITICAL_BIT
}

/// Catalog projection tags (§4.3).
mod catalog_tag {
    /// Display name, critical and required.
    pub const DISPLAY_NAME: u16 = 0x8001;
    /// Retention, critical and required.
    pub const RETENTION: u16 = 0x8002;
    /// Selected, noncritical and optional.
    pub const SELECTED: u16 = 0x0003;
    /// Trusted creation UTC, noncritical and optional.
    pub const CREATED_UTC: u16 = 0x0004;
}

/// Put and patch tags (§4.1, §4.2).
mod request_tag {
    /// Retention, in both the Put and the patch schema.
    pub const RETENTION: u16 = 0x8001;
    /// Selected, patch only.
    pub const SELECTED: u16 = 0x8002;
    /// Display name, patch only.
    pub const DISPLAY_NAME: u16 = 0x8003;
}

/// The facts the route validator derives from one sealed payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteFacts {
    /// The name bytes, at the header's declared length.
    name: [u8; obcr::NAME_CAP],
    name_len: u8,
    /// Stored geometry chunks.
    pub chunks: u32,
    /// Stored route points.
    pub points: u32,
    /// Stored waypoints.
    pub waypoints: u16,
}

impl RouteFacts {
    /// The route's display name, which §4.3 makes the projection's required first field.
    pub fn name(&self) -> &str {
        // Proved UTF-8 by `derive`; a route that reached here without it was refused.
        core::str::from_utf8(&self.name[..usize::from(self.name_len)]).unwrap_or("")
    }
}

/// The resident route repository: the rules, with no state of their own.
///
/// Empty by design. Everything a route mutation needs is either in the request, in the payload, or
/// in the head the store already holds — a repository that cached any of it would be a second place
/// the catalog could be wrong.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RouteRepository;

impl RouteRepository {
    /// The typed validation §7 runs after the seal, and the projection §4.3 requires with it.
    pub fn validate(
        &mut self,
        subject: &Validation<'_>,
        bytes: &mut dyn SealedBytes,
    ) -> Result<CatalogProjection, u16> {
        match subject.opcode {
            Opcode::StartUpload => self.put(subject, bytes),
            Opcode::SetMetadata => self.patch(subject),
            // A delete publishes no head, so there is no projection to derive and nothing about the
            // bytes to judge: the payload it removes was validated when it was published.
            _ => Ok(CatalogProjection::RESERVATION),
        }
    }

    /// A Put: validate the OBCR frame, then project the name it carries and the declared retention.
    fn put(&mut self, subject: &Validation<'_>, bytes: &mut dyn SealedBytes) -> Result<CatalogProjection, u16> {
        let facts = derive(bytes, subject.length)?;
        let declared = MetadataEnvelope::decode(subject.metadata, MAX_REGISTERED_MUTATION_ENVELOPE)
            .map_err(|_| INVALID_ROUTE_FORMAT)?;
        Schema::lookup(ObjectKind::Route, SchemaClass::Put)
            .ok_or(INVALID_ROUTE_FORMAT)?
            .validate(&declared)
            .map_err(|_| INVALID_ROUTE_FORMAT)?;
        let keep =
            declared.field(base(request_tag::RETENTION)).and_then(|field| field.as_u8()).ok_or(INVALID_ROUTE_FORMAT)?;
        project(facts.name(), keep, None, None)
    }

    /// A patch: every present field applied to the projection the head already carries, in one
    /// commit. §4.2's "applied", not "replaced" — a patch that named only retention must not erase
    /// the name the payload gave the head.
    fn patch(&mut self, subject: &Validation<'_>) -> Result<CatalogProjection, u16> {
        let current = subject.current.ok_or(INVALID_ROUTE_FORMAT)?;
        let current = MetadataEnvelope::decode(current, MAX_CATALOG_ENVELOPE).map_err(|_| INVALID_ROUTE_FORMAT)?;
        let patch = MetadataEnvelope::decode(subject.metadata, MAX_REGISTERED_MUTATION_ENVELOPE)
            .map_err(|_| INVALID_ROUTE_FORMAT)?;
        Schema::lookup(ObjectKind::Route, SchemaClass::Patch)
            .ok_or(INVALID_ROUTE_FORMAT)?
            .validate(&patch)
            .map_err(|_| INVALID_ROUTE_FORMAT)?;

        let name = patch
            .field(base(request_tag::DISPLAY_NAME))
            .and_then(|field| field.as_str())
            .or_else(|| current.field(base(catalog_tag::DISPLAY_NAME)).and_then(|field| field.as_str()))
            .ok_or(INVALID_ROUTE_FORMAT)?;
        let keep = patch
            .field(base(request_tag::RETENTION))
            .and_then(|field| field.as_u8())
            .or_else(|| current.field(base(catalog_tag::RETENTION)).and_then(|field| field.as_u8()))
            .ok_or(INVALID_ROUTE_FORMAT)?;
        let selected = patch
            .field(base(request_tag::SELECTED))
            .and_then(|field| field.as_u8())
            .or_else(|| current.field(base(catalog_tag::SELECTED)).and_then(|field| field.as_u8()));
        // Nothing patches the creation time: it is a fact about when the route arrived, and §4.3
        // gives no request schema a field for it. It is carried across untouched.
        let created = current.field(base(catalog_tag::CREATED_UTC)).and_then(|field| field.as_i64());
        project(name, keep, selected, created)
    }
}

/// The §4.3 projection, built canonically and checked against its own registered schema.
fn project(name: &str, keep: u8, selected: Option<u8>, created: Option<i64>) -> Result<CatalogProjection, u16> {
    if name.is_empty() || name.len() > obcr::NAME_CAP || keep > retention::MAX {
        return Err(INVALID_ROUTE_FORMAT);
    }
    let mut buffer = EMPTY_ENVELOPE;
    let mut writer = MetadataWriter::new(&mut buffer).map_err(|_| INVALID_ROUTE_FORMAT)?;
    writer.push(catalog_tag::DISPLAY_NAME, name.as_bytes()).map_err(|_| INVALID_ROUTE_FORMAT)?;
    writer.push(catalog_tag::RETENTION, &[keep]).map_err(|_| INVALID_ROUTE_FORMAT)?;
    if let Some(selected) = selected {
        writer.push(catalog_tag::SELECTED, &[selected]).map_err(|_| INVALID_ROUTE_FORMAT)?;
    }
    if let Some(created) = created {
        writer.push(catalog_tag::CREATED_UTC, &created.to_le_bytes()).map_err(|_| INVALID_ROUTE_FORMAT)?;
    }
    let encoded = writer.finish(ObjectKind::Route, SchemaClass::Catalog);
    // The projection is held to the schema a client will decode it with. A device that stored one
    // its own registry would reject is a device that serves a page no client can read.
    let decoded = MetadataEnvelope::decode(encoded, MAX_CATALOG_ENVELOPE).map_err(|_| INVALID_ROUTE_FORMAT)?;
    Schema::lookup(ObjectKind::Route, SchemaClass::Catalog)
        .ok_or(INVALID_ROUTE_FORMAT)?
        .validate(&decoded)
        .map_err(|_| INVALID_ROUTE_FORMAT)?;
    CatalogProjection::of(encoded).ok_or(INVALID_ROUTE_FORMAT)
}

/// Validates the OBCR frame and derives the facts §4.3 needs, from one 128-byte header read.
fn derive(bytes: &mut dyn SealedBytes, length: u64) -> Result<RouteFacts, u16> {
    if length < obcr::HEADER_FULL_LEN as u64 {
        return Err(INVALID_ROUTE_FORMAT);
    }
    let mut header = [0u8; obcr::HEADER_FULL_LEN];
    match bytes.read_at(0, &mut header) {
        Some(read) if read == obcr::HEADER_FULL_LEN => {}
        // A short read of bytes the seal proved are there, or a medium that refused: either way this
        // is not a payload this device can publish, and the client's remedy is the same.
        _ => return Err(INVALID_ROUTE_FORMAT),
    }
    obcr::validate_header_prefix(&header).map_err(|_| INVALID_ROUTE_FORMAT)?;
    // §1: flags and the byte at offset 7 are reserved zero.
    if header[5] != 0 || header[7] != 0 {
        return Err(INVALID_ROUTE_FORMAT);
    }
    let name_len = header[6];
    if name_len == 0 || usize::from(name_len) > obcr::NAME_CAP {
        return Err(INVALID_ROUTE_FORMAT);
    }
    let name_field = &header[64..64 + obcr::NAME_CAP];
    let (name_bytes, padding) = name_field.split_at(usize::from(name_len));
    // "null-padded": a nonzero byte past the declared length would make two different names encode
    // to one header, exactly the ambiguity §5.3's zero-tail rule forbids in a catalog entry.
    if padding.iter().any(|byte| *byte != 0) || !text_is_clean(name_bytes) {
        return Err(INVALID_ROUTE_FORMAT);
    }
    let points = u32_at(&header, 32);
    let chunks = u32_at(&header, 52);
    let index_offset = u64::from(u32_at(&header, 56));
    let data_offset = u64::from(u32_at(&header, 60));
    let waypoint_offset = u64::from(u32_at(&header, 112));
    let waypoints = u16::from_le_bytes([header[116], header[117]]);
    if header[118..obcr::HEADER_FULL_LEN].iter().any(|byte| *byte != 0) {
        return Err(INVALID_ROUTE_FORMAT);
    }
    if chunks == 0 || points == 0 {
        return Err(INVALID_ROUTE_FORMAT);
    }
    let header_end = obcr::HEADER_FULL_LEN as u64;
    // Every section is reached by an explicit offset, so every offset has to name bytes this file
    // actually has. A reader that trusted one past the end would fault on a head this store
    // published, which is the failure a typed validator exists to make impossible.
    let index_end = index_offset.saturating_add(u64::from(chunks) * obcr::CHUNK_META_LEN as u64);
    if data_offset < header_end || data_offset >= length || index_offset < header_end || index_end > length {
        return Err(INVALID_ROUTE_FORMAT);
    }
    if waypoints == 0 {
        // "`0` when Waypoint Count is 0".
        if waypoint_offset != 0 {
            return Err(INVALID_ROUTE_FORMAT);
        }
    } else {
        let waypoint_end = waypoint_offset.saturating_add(u64::from(waypoints) * obcr::WAYPOINT_LEN as u64);
        if waypoint_offset < header_end || waypoint_end > length {
            return Err(INVALID_ROUTE_FORMAT);
        }
    }
    let mut name = [0u8; obcr::NAME_CAP];
    name[..name_bytes.len()].copy_from_slice(name_bytes);
    Ok(RouteFacts { name, name_len, chunks, points, waypoints })
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// What a route Put wants to do, before anything is claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PutIntent {
    /// Create a new route, or replace one at an exact revision.
    pub target: Target,
    /// The payload length the request declared.
    pub declared_length: u64,
    /// The Put envelope the request declared.
    pub metadata: [u8; MAX_REGISTERED_MUTATION_ENVELOPE],
    /// Its length.
    pub metadata_len: u16,
}

/// What the repository decided about a Put, having created nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PutPlan {
    /// The head the Put will replace, and `None` for a create.
    pub replaces: Option<HeadView>,
    /// The repository revision observed at admission — a diagnostic snapshot, not a token.
    pub repository_revision: obc_link::ids::Revision,
    /// The retention the request declared, already inside §4.1's range.
    pub retention: u8,
}

/// The borrowed route repository: `CardStore` lends the store to it, and takes it back.
pub struct Routes<'a, M: KernelMedia> {
    capability: Capability<'a, M>,
}

impl<'a, M: KernelMedia> Routes<'a, M> {
    /// Wraps a lent capability. Crate-private: `CardStore` is the only lender.
    pub(in crate::obc2) fn new(capability: Capability<'a, M>) -> Self {
        Routes { capability }
    }

    /// §4's repository revision for routes.
    pub fn revision(&self) -> obc_link::ids::Revision {
        self.capability.revision(ObjectKind::Route)
    }

    /// The head a logical route stands at.
    pub fn resolve(&self, logical_object_id: LogicalObjectId) -> Option<HeadView> {
        self.capability.head(ObjectKind::Route, logical_object_id)
    }

    /// One page of routes in logical-ID order, starting after `after`.
    pub fn list(&self, after: Option<LogicalObjectId>, out: &mut [HeadView]) -> usize {
        self.capability.page(ObjectKind::Route, after, out)
    }

    /// How many routes the store holds, against §2's ceiling of [`MAX_ROUTE_HEADS`].
    pub fn count(&self) -> usize {
        self.capability.count(ObjectKind::Route)
    }

    /// §11's preflight for a Put: authorization, schema, capacity, space and the compare-and-swap,
    /// **without creating state**.
    ///
    /// §11 is explicit that this is a distinct step — checks "that intentionally do not claim an
    /// operation" — and that its refusals leave the OperationId reusable. Everything here is
    /// therefore a read: the projection, the free-space figure, and the head's current revision.
    pub fn plan_put(&mut self, intent: &PutIntent) -> Result<PutPlan, FailureCause> {
        let declared = MetadataEnvelope::decode(
            &intent.metadata[..usize::from(intent.metadata_len)],
            MAX_REGISTERED_MUTATION_ENVELOPE,
        )
        .map_err(|_| semantic(INVALID_ROUTE_FORMAT))?;
        let schema = Schema::lookup(ObjectKind::Route, SchemaClass::Put)
            .ok_or(FailureCause::UnsupportedCapability { detail: detail::capability::SCHEMA_VERSION })?;
        schema.validate(&declared).map_err(|_| semantic(INVALID_ROUTE_FORMAT))?;
        let keep = declared
            .field(base(request_tag::RETENTION))
            .and_then(|field| field.as_u8())
            .ok_or_else(|| semantic(INVALID_ROUTE_FORMAT))?;

        if intent.declared_length == 0 || intent.declared_length > MAX_GENERATION_LEN {
            return Err(FailureCause::ResourceLimit { detail: obc_link::error::detail::resource::OBJECT_LENGTH });
        }
        let available = self.capability.free_bytes();
        if intent.declared_length > available {
            return Err(FailureCause::InsufficientSpace { required: intent.declared_length, available });
        }

        let replaces = match intent.target {
            Target::Create => {
                if self.count() >= MAX_ROUTE_HEADS {
                    return Err(FailureCause::ResourceLimit {
                        detail: obc_link::error::detail::resource::CATALOG_HEADS,
                    });
                }
                None
            }
            Target::Replace { logical_object_id, expected_revision } => {
                let head = self.resolve(logical_object_id).ok_or_else(not_found)?;
                if head.revision != expected_revision {
                    return Err(revision_conflict(head.revision));
                }
                Some(head)
            }
        };
        Ok(PutPlan { replaces, repository_revision: self.revision(), retention: keep })
    }

    /// The retention a route's catalog projection carries, re-read from the card.
    ///
    /// This is the metadata policy §4.1 fixes, read back through the one place it is stored. There
    /// is no retention sidecar and no second copy: a route's retention *is* a field of its catalog
    /// projection, which is why changing it is a catalog commit and not a file write.
    pub fn retention(&mut self, logical_object_id: LogicalObjectId) -> Result<Option<u8>, FailureCause> {
        let keep = self.with_projection(logical_object_id, |envelope| {
            envelope.field(base(catalog_tag::RETENTION)).and_then(|field| field.as_u8())
        })?;
        Ok(keep.flatten())
    }

    /// The display name a route's projection carries, copied into `into`.
    pub fn display_name(
        &mut self,
        logical_object_id: LogicalObjectId,
        into: &mut [u8],
    ) -> Result<Option<usize>, FailureCause> {
        let mut staged = [0u8; obcr::NAME_CAP];
        let name = self.with_projection(logical_object_id, |envelope| {
            let field = envelope.field(base(catalog_tag::DISPLAY_NAME))?;
            let bytes = field.as_str()?.as_bytes();
            staged[..bytes.len()].copy_from_slice(bytes);
            Some(bytes.len())
        })?;
        Ok(name.flatten().map(|len| {
            let copied = len.min(into.len());
            into[..copied].copy_from_slice(&staged[..copied]);
            copied
        }))
    }

    /// Whether the route is marked selected, absent when the device holds no such fact.
    pub fn selected(&mut self, logical_object_id: LogicalObjectId) -> Result<Option<bool>, FailureCause> {
        let flag = self.with_projection(logical_object_id, |envelope| {
            envelope.field(base(catalog_tag::SELECTED)).and_then(|field| field.as_u8())
        })?;
        Ok(flag.flatten().map(|value| value != 0))
    }

    /// The head's catalog projection, copied out whole.
    ///
    /// This is what a catalog page is built from, which is why it is here rather than on the store:
    /// the envelope's *meaning* is the repository's, and a caller that wants the bytes should be
    /// asking the repository that wrote them.
    pub fn projection(
        &mut self,
        logical_object_id: LogicalObjectId,
        into: &mut [u8],
    ) -> Result<Option<usize>, FailureCause> {
        self.capability.projection(ObjectKind::Route, logical_object_id, into)
    }

    /// Runs `read` over the head's decoded catalog projection.
    fn with_projection<T>(
        &mut self,
        logical_object_id: LogicalObjectId,
        read: impl FnOnce(&MetadataEnvelope<'_>) -> T,
    ) -> Result<Option<T>, FailureCause> {
        let mut staged = [0u8; MAX_CATALOG_ENVELOPE];
        let Some(len) = self.capability.projection(ObjectKind::Route, logical_object_id, &mut staged)? else {
            return Ok(None);
        };
        let Ok(envelope) = MetadataEnvelope::decode(&staged[..len], MAX_CATALOG_ENVELOPE) else {
            // The head carries §5.3's bare reservation rather than a projection — a head this
            // repository did not publish. It is not an error, and it is not a route with no name.
            return Ok(None);
        };
        Ok(Some(read(&envelope)))
    }
}

/// Wraps a route-namespace semantic detail as the §12 cause the store reports.
fn semantic(detail: u16) -> FailureCause {
    FailureCause::SemanticValidation { kind: ObjectKind::Route, detail }
}

/// A route's `retention` value, projected only to a helper name — the numbers are the registry's.
#[cfg(test)]
fn retention_is_registered(value: u8) -> bool {
    value <= retention::MAX
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_link::registry::semantic;

    /// The one detail this repository may emit is the one the registry allocates for it.
    #[test]
    fn the_route_namespace_has_exactly_the_detail_this_file_emits() {
        let row = semantic::lookup(ObjectKind::Route, INVALID_ROUTE_FORMAT).expect("a registered detail");
        assert_eq!(row.name, "invalidRouteFormat");
        assert!(row.terminal, "a format failure is terminal after the claim");
        assert_eq!(
            semantic::table().filter(|row| row.kind == ObjectKind::Route).count(),
            1,
            "route registers one detail; a second one would need a registry change"
        );
    }

    #[test]
    fn the_advertised_operations_are_the_registry_matrix_row() {
        // §1: route is the only kind with all four subject bits.
        assert_eq!(
            OPERATIONS,
            subject_flags::PUT | subject_flags::GET | subject_flags::DELETE | subject_flags::SET_METADATA
        );
        assert_eq!(OPERATIONS & subject_flags::DRAFT_FINALIZE, 0, "a route is never a draft");
        assert!(retention_is_registered(retention::TWO_MONTHS));
        assert!(!retention_is_registered(retention::MAX + 1));
    }
}
