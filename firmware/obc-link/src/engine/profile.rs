//! The compiled facts a device advertises, and the Capabilities pages built from them (§5).
//!
//! §5 calls these "compiled facts of the running firmware image": the subject registry and the
//! fixed resource limits cannot change while a connection is up, which is exactly why the
//! capability revision is immutable within a connection generation. The engine holds one profile
//! and answers every Hello from it.
//!
//! The profile is also the authority for what the engine admits: an opcode whose command-flag bit
//! is clear is `unsupportedCapability/opcode`, and a kind whose subject entry does not advertise
//! the operation is `unsupportedCapability/logicalKind`. **Resumable upload is deliberately not
//! advertised by any kind in v3.0's first device** — §6.1's restart-only profile — so the engine
//! never reports a durable next offset above zero.

use crate::error::{detail, ErrorBody, ErrorCategory, RetryGuidance};
use crate::frame::Opcode;
use crate::hello::{
    status_flags, Capabilities, CapabilityPage, Hello, LinkKind, PageKind, ResourceLimits, Subject, SubjectEntry,
    MAX_SUBJECTS, SUBJECTS_PER_PAGE,
};
use crate::ids::StoreId;
use crate::metadata::{MAX_CATALOG_ENVELOPE, MAX_PUT_ENVELOPE};
use crate::registry::{subject_flags, ObjectKind};

use super::connection::Negotiated;

/// The subject registry a device compiles in, bounded by §5's sixteen.
#[derive(Debug, Clone, Copy)]
pub struct SubjectTable {
    entries: [Option<SubjectEntry>; MAX_SUBJECTS],
    len: usize,
}

impl Default for SubjectTable {
    fn default() -> Self {
        Self::new()
    }
}

impl SubjectTable {
    /// An empty registry. A device with one is conforming: §5 makes zero subjects a legal page.
    pub const fn new() -> Self {
        SubjectTable { entries: [None; MAX_SUBJECTS], len: 0 }
    }

    /// Appends one entry, keeping the ascending `(namespace, kind_code)` order §5 requires.
    ///
    /// Returns `false` when the registry is full or the entry is out of order.
    pub fn push(&mut self, entry: SubjectEntry) -> bool {
        if self.len == MAX_SUBJECTS {
            return false;
        }
        if let Some(Some(last)) = self.len.checked_sub(1).map(|index| self.entries[index]) {
            if entry.subject.sort_key() <= last.subject.sort_key() {
                return false;
            }
        }
        self.entries[self.len] = Some(entry);
        self.len += 1;
        true
    }

    /// How many subjects the registry holds.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// True when the device advertises no subject at all.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The entry for one subject, if it is advertised.
    pub fn entry(&self, subject: Subject) -> Option<SubjectEntry> {
        self.entries.iter().flatten().find(|entry| entry.subject == subject).copied()
    }

    /// The page of at most two entries §5 puts at `page_index`.
    pub fn page(&self, page_index: u8) -> Option<[Option<SubjectEntry>; SUBJECTS_PER_PAGE]> {
        let first = usize::from(page_index) * SUBJECTS_PER_PAGE;
        if first >= self.len && page_index > 0 {
            return None;
        }
        let mut page = [None; SUBJECTS_PER_PAGE];
        for (slot, index) in page.iter_mut().zip(first..first + SUBJECTS_PER_PAGE) {
            *slot = self.entries.get(index).copied().flatten();
        }
        Some(page)
    }

    /// The total number of subject pages: `ceil(len / 2)`, and zero when nothing is advertised.
    pub const fn total_pages(&self) -> u8 {
        self.len.div_ceil(SUBJECTS_PER_PAGE) as u8
    }
}

/// The compiled device facts the engine answers from.
#[derive(Debug, Clone, Copy)]
pub struct DeviceProfile {
    /// The mounted store's identity, or zero when no store is available.
    pub store_id: StoreId,
    /// Whether a store is available at all (§5 status bit 0).
    pub store_available: bool,
    /// Whether developer/unlocked mode is on (§5 status bit 3).
    pub developer_unlocked: bool,
    /// The capability snapshot's revision. Immutable within a connection generation.
    pub capability_revision: u32,
    /// Which §16 operations this device serves.
    pub command_flags: u32,
    /// The durable checkpoint granule this device uses (§6.2).
    pub checkpoint_granule: u32,
    /// The largest control frame the device itself can serve.
    pub device_max_control_frame: u16,
    /// The largest stream frame the device itself can serve.
    pub device_max_stream_frame: u16,
    /// The fixed product/storage capacities of §5.1.
    pub limits: ResourceLimits,
    /// The advertised subjects.
    pub subjects: SubjectTable,
    /// The device's wire minor within the selected major (§5, byte 55).
    pub device_wire_minor: u8,
}

impl DeviceProfile {
    /// A profile with no subjects, no store, and no device-control plane.
    pub const fn new(store_id: StoreId) -> Self {
        DeviceProfile {
            store_id,
            store_available: true,
            developer_unlocked: false,
            capability_revision: 1,
            command_flags: 0,
            checkpoint_granule: crate::hello::DEFAULT_CHECKPOINT_GRANULE,
            device_max_control_frame: crate::frame::MAX_CONTROL_FRAME as u16,
            device_max_stream_frame: crate::frame::MAX_STREAM_FRAME as u16,
            limits: ResourceLimits::REFERENCE,
            subjects: SubjectTable::new(),
            device_wire_minor: crate::WIRE_MINOR,
        }
    }

    /// True when the device serves this opcode.
    ///
    /// Opcodes with no command-flag bit — Hello and the transfer plane — are always served: §5
    /// gates only the seventeen it names, and "a device that cannot answer those is not speaking
    /// this protocol at all".
    pub fn serves(&self, opcode: Opcode) -> bool {
        match opcode.command_flag() {
            Some(bit) => self.command_flags & bit != 0,
            None => true,
        }
    }

    /// The subject entry for a logical kind, when the device advertises it.
    pub fn logical(&self, kind: ObjectKind) -> Option<SubjectEntry> {
        self.subjects.entry(Subject::Logical(kind))
    }

    /// True when the kind advertises resumable upload, which the restart-only profile never does.
    pub fn advertises_resumable_upload(&self, kind: ObjectKind) -> bool {
        self.logical(kind).is_some_and(|entry| entry.operation_flags & subject_flags::RESUMABLE_UPLOAD != 0)
    }

    /// True when the kind advertises resumable download, which a nonzero start offset requires.
    pub fn advertises_resumable_download(&self, kind: ObjectKind) -> bool {
        self.logical(kind).is_some_and(|entry| entry.operation_flags & subject_flags::RESUMABLE_DOWNLOAD != 0)
    }

    /// Checks that the kind is advertised and serves `operation`, or names the refusal.
    pub fn require_operation(&self, kind: ObjectKind, operation: u16) -> Result<SubjectEntry, ErrorBody<'static>> {
        match self.logical(kind) {
            Some(entry) if entry.operation_flags & operation != 0 => Ok(entry),
            _ => Err(ErrorBody::bare(
                ErrorCategory::UNSUPPORTED_CAPABILITY,
                detail::capability::LOGICAL_KIND,
                RetryGuidance::REJECT_PERMANENTLY,
            )),
        }
    }

    /// Builds the Capabilities page a Hello asks for, or names why the request is illegal.
    ///
    /// `heavy_transfer_busy` is the one ephemeral status flag the engine knows and the profile does
    /// not: whether a heavy transfer currently owns the coordinator.
    pub fn capabilities(
        &self,
        hello: &Hello,
        negotiated: &Negotiated,
        link_kind: LinkKind,
        authenticated: bool,
        heavy_transfer_busy: bool,
    ) -> Result<(Capabilities, bool), ErrorBody<'static>> {
        let mut status = 0;
        if self.store_available {
            status |= status_flags::STORE_AVAILABLE;
        }
        if authenticated {
            status |= status_flags::AUTHENTICATED;
        }
        if heavy_transfer_busy {
            status |= status_flags::HEAVY_TRANSFER_BUSY;
        }
        if self.developer_unlocked {
            status |= status_flags::DEVELOPER_UNLOCKED;
        }
        let total_subject_count = self.subjects.len() as u16;
        let (page, page_index, more) = match hello.page_kind {
            PageKind::Resources => {
                if hello.page_index != 0 {
                    // §5: "A nonzero resource-page index ... is `invalidDescriptor`."
                    return Err(descriptor_refusal());
                }
                (CapabilityPage::Resources(self.limits), 0, total_subject_count > 0)
            }
            PageKind::Subjects => {
                let page = self.subjects.page(hello.page_index).ok_or_else(descriptor_refusal)?;
                // The wire page is a fixed pair with a count; a slot past the count is never
                // encoded, so an unused one carries the padding entry below rather than an Option.
                let mut entries = [PAGE_PADDING; SUBJECTS_PER_PAGE];
                let mut count = 0u8;
                for (slot, entry) in entries.iter_mut().zip(page) {
                    if let Some(entry) = entry {
                        *slot = entry;
                        count += 1;
                    }
                }
                let more = u16::from(hello.page_index) + 1 < u16::from(self.subjects.total_pages());
                (CapabilityPage::Subjects { entries, count }, hello.page_index, more)
            }
        };
        let capabilities = Capabilities {
            selected_major: crate::WIRE_MAJOR,
            storage_format_version: crate::STORAGE_FORMAT_VERSION,
            status_flags: status,
            store_id: if self.store_available { self.store_id } else { StoreId::ZERO },
            negotiated_control_frame: negotiated.control_frame,
            negotiated_stream_frame: negotiated.stream_frame,
            checkpoint_granule: self.checkpoint_granule,
            retained_result_capacity: crate::hello::RETAINED_RESULT_CAPACITY,
            metadata_envelope_limit: MAX_PUT_ENVELOPE as u16,
            catalog_metadata_limit: MAX_CATALOG_ENVELOPE as u16,
            protocol_min_control_frame: crate::frame::MIN_CONTROL_FRAME as u16,
            protocol_min_stream_frame: crate::frame::MIN_STREAM_FRAME as u16,
            link_kind,
            authenticated,
            capability_revision: self.capability_revision,
            command_flags: self.command_flags,
            total_subject_count,
            page_index,
            total_pages: match hello.page_kind {
                PageKind::Resources => 1,
                PageKind::Subjects => self.subjects.total_pages(),
            },
            device_wire_minor: self.device_wire_minor,
            page,
        };
        Ok((capabilities, more))
    }
}

/// The value an unused slot of a subject page carries. It is never encoded: §5's page is `count`
/// entries and the codec writes exactly that many.
const PAGE_PADDING: SubjectEntry = SubjectEntry {
    subject: Subject::Logical(ObjectKind::Route),
    operation_flags: 0,
    policy_flags: 0,
    put_schema_version: 0,
    patch_schema_version: 0,
    catalog_schema_version: 0,
    max_length: 0,
};

fn descriptor_refusal() -> ErrorBody<'static> {
    ErrorBody::bare(
        ErrorCategory::INVALID_DESCRIPTOR,
        detail::descriptor::INVALID_COMBINATION,
        RetryGuidance::REJECT_PERMANENTLY,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::schema_version;

    fn route_subject() -> SubjectEntry {
        SubjectEntry {
            subject: Subject::Logical(ObjectKind::Route),
            operation_flags: subject_flags::PUT
                | subject_flags::GET
                | subject_flags::DELETE
                | subject_flags::SET_METADATA,
            policy_flags: 0,
            put_schema_version: schema_version::PUT,
            patch_schema_version: schema_version::PATCH,
            catalog_schema_version: schema_version::CATALOG,
            max_length: 1 << 20,
        }
    }

    fn ride_subject() -> SubjectEntry {
        SubjectEntry {
            subject: Subject::Logical(ObjectKind::Ride),
            operation_flags: subject_flags::GET | subject_flags::DELETE,
            policy_flags: 0,
            put_schema_version: 0,
            patch_schema_version: 0,
            catalog_schema_version: schema_version::CATALOG,
            max_length: 1 << 20,
        }
    }

    fn hello(page_kind: PageKind, page_index: u8) -> Hello {
        Hello {
            minimum_major: 3,
            maximum_major: 3,
            client_max_control_frame: 244,
            client_max_stream_frame: 1_024,
            client_feature_flags: 0,
            page_kind,
            page_index,
        }
    }

    fn negotiated() -> Negotiated {
        Negotiated { hello: hello(PageKind::Resources, 0), control_frame: 244, stream_frame: 1_024 }
    }

    fn profile() -> DeviceProfile {
        let mut profile = DeviceProfile::new(StoreId::new([9; 16]));
        assert!(profile.subjects.push(route_subject()));
        assert!(profile.subjects.push(ride_subject()));
        profile
    }

    #[test]
    fn the_registry_keeps_ascending_order_and_its_bound() {
        let mut table = SubjectTable::new();
        assert!(table.push(ride_subject()));
        assert!(!table.push(route_subject()), "route sorts below ride and cannot follow it");
        assert_eq!(table.len(), 1);
        assert_eq!(table.total_pages(), 1);
    }

    #[test]
    fn the_resource_page_sets_more_exactly_when_subjects_exist() {
        let profile = profile();
        let (page, more) =
            profile.capabilities(&hello(PageKind::Resources, 0), &negotiated(), LinkKind::Ble, true, false).unwrap();
        assert!(more);
        assert_eq!(page.total_subject_count, 2);
        assert_eq!(page.total_pages, 1);
        assert!(matches!(page.page, CapabilityPage::Resources(_)));

        let bare = DeviceProfile::new(StoreId::new([1; 16]));
        let (page, more) =
            bare.capabilities(&hello(PageKind::Resources, 0), &negotiated(), LinkKind::Usb, true, false).unwrap();
        assert!(!more);
        assert_eq!(page.total_subject_count, 0);
    }

    #[test]
    fn a_subject_page_beyond_the_last_is_an_invalid_descriptor() {
        let profile = profile();
        let (page, more) =
            profile.capabilities(&hello(PageKind::Subjects, 0), &negotiated(), LinkKind::Ble, true, true).unwrap();
        assert!(!more);
        assert_eq!(page.page.entries().len(), 2);
        assert_ne!(page.status_flags & status_flags::HEAVY_TRANSFER_BUSY, 0);

        let refusal =
            profile.capabilities(&hello(PageKind::Subjects, 1), &negotiated(), LinkKind::Ble, true, false).unwrap_err();
        assert_eq!(refusal.category, ErrorCategory::INVALID_DESCRIPTOR);
        let refusal = profile
            .capabilities(&hello(PageKind::Resources, 1), &negotiated(), LinkKind::Ble, true, false)
            .unwrap_err();
        assert_eq!(refusal.category, ErrorCategory::INVALID_DESCRIPTOR);
    }

    #[test]
    fn a_zero_subject_device_answers_page_zero_and_rejects_page_one() {
        let bare = DeviceProfile::new(StoreId::new([1; 16]));
        let (page, more) =
            bare.capabilities(&hello(PageKind::Subjects, 0), &negotiated(), LinkKind::Test, false, false).unwrap();
        assert!(!more);
        assert_eq!(page.total_pages, 0);
        assert!(page.page.entries().is_empty());
        assert!(bare.capabilities(&hello(PageKind::Subjects, 1), &negotiated(), LinkKind::Test, false, false).is_err());
    }

    #[test]
    fn an_unadvertised_operation_or_kind_is_an_unsupported_capability() {
        let profile = profile();
        assert!(profile.require_operation(ObjectKind::Route, subject_flags::PUT).is_ok());
        let refusal = profile.require_operation(ObjectKind::Ride, subject_flags::PUT).unwrap_err();
        assert_eq!(refusal.category, ErrorCategory::UNSUPPORTED_CAPABILITY);
        assert_eq!(refusal.detail, detail::capability::LOGICAL_KIND);
        let refusal = profile.require_operation(ObjectKind::Weather, subject_flags::GET).unwrap_err();
        assert_eq!(refusal.detail, detail::capability::LOGICAL_KIND);
    }

    #[test]
    fn the_restart_only_profile_advertises_no_resumable_upload() {
        let profile = profile();
        for kind in ObjectKind::ALL {
            assert!(!profile.advertises_resumable_upload(kind), "{} advertises resumable upload", kind.name());
        }
    }

    #[test]
    fn a_cleared_command_flag_means_the_device_does_not_serve_the_opcode() {
        let mut profile = profile();
        assert!(!profile.serves(Opcode::Echo));
        assert!(profile.serves(Opcode::StartUpload), "the transfer plane has no gating bit");
        profile.command_flags = Opcode::Echo.command_flag().unwrap();
        assert!(profile.serves(Opcode::Echo));
        assert!(!profile.serves(Opcode::ResetStore));
    }
}
