//! The weather repository — shaped, with the half of its projection that is already frozen.
//!
//! Weather is the sharpest argument against a shared repository trait, which is why it is the third
//! shape in this slice rather than a later one. Everything about it differs from a route:
//!
//! - its identity is a **store-owned singleton** reserved at initialization (§3), so there is no
//!   create and no logical ID a client may choose;
//! - its catalog projection (§4.3) is three timestamps and a `WeatherRequestId` — no name, no
//!   retention, nothing a person types;
//! - those three facts arrive in the **Put envelope** (§3.1), not in the payload's header, so the
//!   projection is derived from a completely different source than a route's;
//! - it has five registered semantic details where a route has one, and its publish rule is a
//!   predicate over a durable request context nothing else in the system has.
//!
//! ## What it derives now, and the one check it does not yet make
//!
//! §3.1 gives the six Put fields and the projection three of them, so the projection is derivable
//! today and this file derives it. It is the honest half of the contract: the values a head carries
//! are the values the client declared, and the schema has already held each of them to §3's ranges
//! and to "valid-until later than issued".
//!
//! **The other half is not here, and it is not hidden.** §3.1: "The typed weather validator MUST
//! derive the same facts from the payload; any mismatch is `weather.payloadFactsMismatch`." Doing
//! that means reading OBCW, and doing the publish rule means reading the durable request context —
//! "a bundle publishes if and only if its compare-and-swap revision still matches **and** its
//! context matches the current request". Both are DOS9's, along with the request context itself,
//! which no part of this store yet holds. Until then this repository declares a projection and
//! declines to claim it verified the bundle.

use obc_link::frame::Opcode;
use obc_link::ids::{LogicalObjectId, Revision, WeatherRequestId};
use obc_link::metadata::{
    MetadataEnvelope, MetadataWriter, Schema, SchemaClass, MAX_CATALOG_ENVELOPE, MAX_REGISTERED_MUTATION_ENVELOPE,
};
use obc_link::registry::{subject_flags, ObjectKind};

use crate::obc2::transaction::{CatalogProjection, KernelMedia, SealedBytes, Validation};

use super::{Capability, HeadView, EMPTY_ENVELOPE};

/// `Device_Object_Registries_v2.md` §6's weather details.
pub mod detail {
    /// Registered, reserved, and never emitted — a stale bundle is `REQUEST_MISMATCH`.
    pub const SUPERSEDED_NOT_USEFUL: u16 = 1;
    /// The bundle's coverage does not satisfy the request.
    pub const COVERAGE_MISMATCH: u16 = 2;
    /// The bundle is no longer valid for long enough.
    pub const STALE_BUNDLE: u16 = 3;
    /// The payload's own facts disagree with the declared ones.
    pub const PAYLOAD_FACTS_MISMATCH: u16 = 4;
    /// The bundle answers a request that is not the current one.
    pub const REQUEST_MISMATCH: u16 = 5;
}

/// The subject operation flags §1's matrix permits weather.
pub const OPERATIONS: u16 = subject_flags::PUT | subject_flags::GET | subject_flags::DELETE;

/// The base tag a full tag names (§2.2: the critical bit is encoding, not identity).
const fn base(tag: u16) -> u16 {
    tag & !obc_link::metadata::CRITICAL_BIT
}

/// §3.1's Put tags.
mod put_tag {
    pub const REQUEST_ID: u16 = 0x8001;
    pub const ISSUED_UTC: u16 = 0x8005;
    pub const VALID_UNTIL_UTC: u16 = 0x8006;
}

/// §4.3's catalog tags.
mod catalog_tag {
    pub const REQUEST_ID: u16 = 0x8001;
    pub const ISSUED_UTC: u16 = 0x8002;
    pub const VALID_UNTIL_UTC: u16 = 0x8003;
}

/// The resident weather repository.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WeatherRepository;

impl WeatherRepository {
    /// The typed validation seam.
    pub fn validate(
        &mut self,
        subject: &Validation<'_>,
        _bytes: &mut dyn SealedBytes,
    ) -> Result<CatalogProjection, u16> {
        match subject.opcode {
            Opcode::StartUpload => self.put(subject),
            // §1's matrix gives weather no set-metadata bit, so a conforming profile refuses the
            // request before a claim exists. It is reachable only by a device advertising a bit the
            // registry forbids, and the answer to that is a refusal — but `payloadFactsMismatch` is
            // a poor description of it, and it is used because the registry allocates weather no
            // detail for "this operation does not exist here". The honest reading: the profile is
            // the fix; this arm exists so the wrong profile cannot silently publish.
            Opcode::SetMetadata => Err(detail::PAYLOAD_FACTS_MISMATCH),
            _ => Ok(CatalogProjection::RESERVATION),
        }
    }

    /// §4.3's three fields, carried across from the six §3.1 declares.
    fn put(&mut self, subject: &Validation<'_>) -> Result<CatalogProjection, u16> {
        let declared = MetadataEnvelope::decode(subject.metadata, MAX_REGISTERED_MUTATION_ENVELOPE)
            .map_err(|_| detail::PAYLOAD_FACTS_MISMATCH)?;
        Schema::lookup(ObjectKind::Weather, SchemaClass::Put)
            .ok_or(detail::PAYLOAD_FACTS_MISMATCH)?
            .validate(&declared)
            .map_err(|_| detail::PAYLOAD_FACTS_MISMATCH)?;
        let request = declared
            .field(base(put_tag::REQUEST_ID))
            .and_then(|field| field.as_u64())
            .ok_or(detail::PAYLOAD_FACTS_MISMATCH)?;
        let issued = declared
            .field(base(put_tag::ISSUED_UTC))
            .and_then(|field| field.as_i64())
            .ok_or(detail::PAYLOAD_FACTS_MISMATCH)?;
        let valid_until = declared
            .field(base(put_tag::VALID_UNTIL_UTC))
            .and_then(|field| field.as_i64())
            .ok_or(detail::PAYLOAD_FACTS_MISMATCH)?;

        let mut buffer = EMPTY_ENVELOPE;
        let mut writer = MetadataWriter::new(&mut buffer).map_err(|_| detail::PAYLOAD_FACTS_MISMATCH)?;
        writer.push(catalog_tag::REQUEST_ID, &request.to_le_bytes()).map_err(|_| detail::PAYLOAD_FACTS_MISMATCH)?;
        writer.push(catalog_tag::ISSUED_UTC, &issued.to_le_bytes()).map_err(|_| detail::PAYLOAD_FACTS_MISMATCH)?;
        writer
            .push(catalog_tag::VALID_UNTIL_UTC, &valid_until.to_le_bytes())
            .map_err(|_| detail::PAYLOAD_FACTS_MISMATCH)?;
        let encoded = writer.finish(ObjectKind::Weather, SchemaClass::Catalog);
        let decoded =
            MetadataEnvelope::decode(encoded, MAX_CATALOG_ENVELOPE).map_err(|_| detail::PAYLOAD_FACTS_MISMATCH)?;
        Schema::lookup(ObjectKind::Weather, SchemaClass::Catalog)
            .ok_or(detail::PAYLOAD_FACTS_MISMATCH)?
            .validate(&decoded)
            .map_err(|_| detail::PAYLOAD_FACTS_MISMATCH)?;
        CatalogProjection::of(encoded).ok_or(detail::PAYLOAD_FACTS_MISMATCH)
    }
}

/// The borrowed weather repository.
pub struct Weather<'a, M: KernelMedia> {
    capability: Capability<'a, M>,
}

impl<'a, M: KernelMedia> Weather<'a, M> {
    /// Wraps a lent capability.
    pub(in crate::obc2) fn new(capability: Capability<'a, M>) -> Self {
        Weather { capability }
    }

    /// §4's repository revision for weather.
    pub fn revision(&self) -> Revision {
        self.capability.revision(ObjectKind::Weather)
    }

    /// §3's reserved singleton identity, which the store persists "even when no weather head
    /// exists".
    ///
    /// It is an ordinary `u64` and clients "never reject the value the device reports — including
    /// zero, which every real device reports today".
    pub fn singleton(&self) -> Option<LogicalObjectId> {
        self.capability.weather_singleton()
    }

    /// The current bundle's head, when there is one.
    pub fn head(&self) -> Option<HeadView> {
        let singleton = self.singleton()?;
        self.capability.head(ObjectKind::Weather, singleton)
    }

    /// The `WeatherRequestId` the current head answered, from its catalog projection.
    pub fn answered_request(&mut self) -> Result<Option<WeatherRequestId>, obc_link::engine::FailureCause> {
        let Some(singleton) = self.singleton() else { return Ok(None) };
        let mut staged = [0u8; MAX_CATALOG_ENVELOPE];
        let Some(len) = self.capability.projection(ObjectKind::Weather, singleton, &mut staged)? else {
            return Ok(None);
        };
        let Ok(envelope) = MetadataEnvelope::decode(&staged[..len], MAX_CATALOG_ENVELOPE) else { return Ok(None) };
        Ok(envelope.field(base(catalog_tag::REQUEST_ID)).and_then(|field| field.as_u64()).map(WeatherRequestId::new))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_link::registry::semantic;

    #[test]
    fn the_weather_namespace_matches_the_registry_and_the_reserved_row_stays_reserved() {
        for (code, name) in [
            (detail::SUPERSEDED_NOT_USEFUL, "supersededNotUseful"),
            (detail::COVERAGE_MISMATCH, "coverageMismatch"),
            (detail::STALE_BUNDLE, "staleBundle"),
            (detail::PAYLOAD_FACTS_MISMATCH, "payloadFactsMismatch"),
            (detail::REQUEST_MISMATCH, "requestMismatch"),
        ] {
            assert_eq!(semantic::lookup(ObjectKind::Weather, code).expect("registered").name, name);
        }
        let reserved = semantic::lookup(ObjectKind::Weather, detail::SUPERSEDED_NOT_USEFUL).expect("registered");
        assert!(reserved.reserved, "a v3.0 device never emits it, and nothing in this file does");
        assert_eq!(OPERATIONS & subject_flags::SET_METADATA, 0);
    }
}
