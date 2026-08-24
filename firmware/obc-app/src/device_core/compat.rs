//! The **legacy compatibility adapter** — one temporary translator between DeviceCore's slots and
//! the old host protocol (#1439, epic #1433 §11).
//!
//! DeviceCore's [`pass`](super::pass) speaks bounded effects and token-carrying outcomes. Every
//! runtime host still speaks [`HostCommand`] and [`HostEvent`]. This module is the one place those
//! two vocabularies meet, so the pass can run *before* the board and host executors migrate — and
//! it is written to be deleted, not extended.
//!
//! ```text
//!   PassPlan::effects       ──effects_to_commands──▶ HostMailbox ──▶ the existing executor
//!   PassPlan::derived_needs ──needs_to_commands───▶ HostMailbox
//!                                                        │
//!   PassInputs ◀── event_to_inputs / feed_ride_track ──── HostEvent, bulk feeder answers
//! ```
//!
//! ## Translation only
//!
//! The adapter decides nothing. It holds **one correlation slot per reply-producing legacy class**
//! ([`LegacyPending`]) — the operation token the old protocol had no field for — plus the store
//! revision the old protocol never reported. That is the complete list of what it remembers. No
//! retry, no cancellation, no replacement rule, no visible state: those belong to the domain that
//! owns the lifecycle, and an adapter that kept any of them would be the duplicate product policy
//! the epic exists to remove.
//!
//! This is also why a *late* answer needs no rule here. The adapter hands the stored token straight
//! back to the domain that minted it; a domain that has since cancelled or replaced its operation
//! rejects that token itself, inside the pass.
//!
//! Intents never pass through. A rider's delete is a `CatalogIntent` that reaches `CatalogMachine`
//! inside the pass; only the *effect* the domain then decides on can become a legacy command.
//!
//! ## Two tables, because the legacy protocol answers only half of it
//!
//! Four domains have a legacy command whose [`HostEvent`] is its terminal answer
//! ([`AnsweredRow`]). The other five do not: their legacy commands are answered by a bulk re-feed,
//! by a store-changed edge, or by nothing at all ([`UnansweredRow`]). Splitting the tables by
//! return type is what makes "does this domain get a token back?" a compile-time fact rather than a
//! comment.
//!
//! ## What the legacy protocol cannot say
//!
//! The old protocol is coarser than the new one in ten specific places — a namespaced delete where
//! the store now removes by object identity, a plan command that acquires, steps and commits in
//! one, a stamp that is never acknowledged. Each is a named [`LegacyOwned`] row that says what the
//! legacy side still owns and which slice deletes the row, and every row a translation hits comes
//! back in the [`LegacyReport`], so nothing is silently skipped.
//!
//! ## One operation per domain, and why
//!
//! **This path completes at most one operation for each of `CatalogMachine`, `RetentionMachine` and
//! `WeatherDomain` — for the whole life of the device.** It is the honest cost of the migration
//! being half done, and it is worth stating rather than discovering.
//!
//! All three latch an in-flight marker the moment they emit an effect, and clear it only in their
//! `apply_outcome`. The adapter can build a `NavigatorOutcome`, a `SettingsOutcome`, a `DfuOutcome`
//! and a `StorageInfoOutcome`, because those four legacy commands have a terminal [`HostEvent`]. It
//! can build **no** catalog, retention or weather outcome, because the legacy protocol answers those
//! commands with a bulk re-feed, a store-changed edge, or nothing at all. Synthesising one anyway —
//! reading a `StoreChanged` as "the delete I asked for finished" — would be the adapter deciding a
//! product rule from an unrelated fact, which is exactly what it must not do.
//!
//! So the latch stays, and it stays whether the effect was translated or not: a *successful*
//! `StampRouteUsed` wedges retention exactly as hard as an untranslatable `RemoveObject` wedges the
//! catalog. #1397 S6 clears it, by giving those domains executors that answer.
//!
//! ## What "put back" does and does not mean
//!
//! An effect the adapter cannot send is put back into the [`EffectSlots`] it came from. That
//! preserves it **for the caller**, which is what makes staging one plan across several calls
//! lossless — the board's spine translates a domain at a time before its first await, and a full
//! mailbox on the first step must not lose an effect by the third.
//!
//! It does **not** hand the effect back to the domain that decided it.
//! [`PassPlan::effects`](super::pass::PassPlan) is pass *output*, built fresh every pass, so the
//! domain will not offer it again — see above for why. The genuinely caller-owned half of the seam
//! is [`PassInputs::outcomes`](super::pass::PassInputs), which does survive to the next pass.
//!
//! ## Bulk stays where it is
//!
//! Catalogs, profiles and polylines enter neither protocol. The two derived levels reach the old
//! feeders through [`feed_ride_track`](LegacyAdapter::feed_ride_track) and
//! [`feed_nav_preview`](LegacyAdapter::feed_nav_preview), whose whole job is to attach the complete
//! data key the legacy `set_*` call omitted. They carry no token: a level is guarded by its key
//! (see [`derived`](super::derived)).

use crate::ble::BondEffect;
use crate::catalog_state::CatalogEffect;
use crate::device_core::derived::{DerivedInput, DerivedNeeds, DerivedResult, NavPreviewKey, RideTrackKey};
use crate::device_core::storage_info::{StorageInfoEffect, StorageInfoError, StorageInfoOutcome};
use crate::device_core::{
    DerivedInputs, DfuTag, EffectSlots, ExternalFacts, FactMergeError, NavigatorTag, OperationToken, OutcomeSlots,
    Revision, RouteUpload, SettingsTag, Slot, StorageInfoTag, StoreIdentity, StoreRevision, TripUpload, UpdateResult,
};
use crate::dfu::{DfuEffect, DfuOutcome};
use crate::navigator::{NavigatorEffect, NavigatorError, NavigatorOutcome, PlannerWork};
use crate::recorder::RecorderEffect;
use crate::retention::RetentionEffect;
use crate::settings::{SettingsEffect, SettingsOutcome};
use crate::weather::WeatherEffect;
use crate::{DfuAction, HostCommand, HostEvent, HostMailbox, TrackAction};

// ==================== the reply classes ====================

/// A legacy class that produces a reply, and therefore needs a correlation slot.
///
/// Seven, not eighteen: most legacy commands are answered by a bulk re-feed, by a store-changed
/// edge, or by nothing at all. Only these seven have a [`HostEvent`] that is the terminal answer to
/// one request, so only these seven have a token to remember.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LegacyReply {
    /// [`HostCommand::PlanRoute`] → [`HostEvent::NavPlanned`].
    RoutePlan,
    /// [`HostCommand::PlanDetour`] → [`HostEvent::DetourPlanned`].
    DetourPlan,
    /// [`HostCommand::CommitDetour`] → [`HostEvent::DetourCommitted`].
    DetourCommit,
    /// [`HostCommand::PersistSettings`] → [`HostEvent::SettingsPersisted`] or `SettingsPersistFailed`.
    SettingsWrite,
    /// [`DfuAction::Scan`] → [`HostEvent::DfuScanned`].
    DfuScan,
    /// [`DfuAction::Install`] → [`HostEvent::DfuInstallBegan`] or `DfuInstallFailed`.
    DfuInstall,
    /// [`HostCommand::ScanCardFree`] → [`HostEvent::CardScanned`].
    CardScan,
}

impl LegacyReply {
    /// Every reply class, in declaration order.
    pub const ALL: [LegacyReply; 7] = [
        LegacyReply::RoutePlan,
        LegacyReply::DetourPlan,
        LegacyReply::DetourCommit,
        LegacyReply::SettingsWrite,
        LegacyReply::DfuScan,
        LegacyReply::DfuInstall,
        LegacyReply::CardScan,
    ];
}

// ==================== what the legacy side still owns ====================

/// One behaviour the legacy protocol keeps because it cannot express the new one.
///
/// Every row names what the old side owns and, through [`deletes_in`](Self::deletes_in), the slice
/// that removes it. A row is not a TODO: it states that the *legacy* protocol is the coarser of the
/// two here, and it disappears with the executor that still needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LegacyOwned {
    /// The legacy protocol has no store revision. `StoreChanged` says the store moved but not to
    /// what, and a rescan answers with bulk feeders rather than a revision, so no
    /// `CatalogOutcome::CatalogRead` can be built from it. The adapter synthesises a monotonic
    /// revision under one fixed legacy store identity, so a commit is still an *edge*.
    StoreRevision,
    /// The legacy delete commands are namespaced (`DeleteRoute` / `DeleteRide` / `DeleteTrip`);
    /// `CatalogEffect::RemoveObject` deliberately is not, because the flat store removes an object
    /// by identity. The namespace cannot be recovered from the effect.
    ObjectNamespace,
    /// The legacy host runs the whole trip cascade inside one `DeleteTrip`, so nothing asks it for
    /// a trip's members.
    TripCascade,
    /// The legacy stamp commands are fire-and-forget: the sidecar write is never acknowledged.
    SidecarAck,
    /// The legacy host writes ride samples and checkpoints off-protocol, from its own session
    /// state.
    RecorderJournal,
    /// The legacy `FinishTrack` is answered by a catalog re-feed, not by a terminal ride identity.
    RideCloseAck,
    /// One legacy `PlanRoute` acquires, paces and commits the search inside the host, so the
    /// per-step and commit effects have no command of their own.
    PlannerPacing,
    /// The legacy host releases the planner workspace and its sources implicitly. A `Release` is
    /// issued on success *and* on cancellation, so it must never be translated into a cancel.
    PlannerRelease,
    /// The legacy bond removal is confirmed by a link-status fact, not by a reply.
    BondAck,
    /// The legacy protocol has no weather command at all — weather still arrives on the companion
    /// feed.
    WeatherProtocol,
}

impl LegacyOwned {
    /// Every row, in declaration order.
    pub const ALL: [LegacyOwned; 10] = [
        LegacyOwned::StoreRevision,
        LegacyOwned::ObjectNamespace,
        LegacyOwned::TripCascade,
        LegacyOwned::SidecarAck,
        LegacyOwned::RecorderJournal,
        LegacyOwned::RideCloseAck,
        LegacyOwned::PlannerPacing,
        LegacyOwned::PlannerRelease,
        LegacyOwned::BondAck,
        LegacyOwned::WeatherProtocol,
    ];

    /// The slice that deletes this row, as prose — the same convention
    /// [`migration`](super::migration) uses, and for the same reason: a slice is an issue, not a
    /// symbol.
    pub const fn deletes_in(self) -> &'static str {
        match self {
            LegacyOwned::StoreRevision | LegacyOwned::ObjectNamespace | LegacyOwned::SidecarAck => {
                "#1397 S6a/S6b — the store and retention executors report revisions and results"
            }
            // Not the store executors': `CatalogState::admit_intent` refuses a trip cascade
            // outright, so there is no bounded member read for one to serve.
            LegacyOwned::TripCascade => "#1491 — the catalog cascade slice builds the bounded member read",
            LegacyOwned::RecorderJournal | LegacyOwned::RideCloseAck => {
                "#1398 — the recorder domain owns the journal and answers with the ride identity"
            }
            LegacyOwned::BondAck => "#1398/#1400 — the bond domain answers its own removal",
            LegacyOwned::PlannerPacing | LegacyOwned::PlannerRelease => {
                "#1400 — the board's typed effect staging paces the planner and answers every release"
            }
            LegacyOwned::WeatherProtocol => "#1401 — the weather storage and request cutover, after FS7",
        }
    }
}

/// A bounded set of [`LegacyOwned`] rows — which ones one translation pass hit, without allocating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LegacyOwnedSet(u16);

impl LegacyOwnedSet {
    /// Nothing hit.
    pub const NONE: LegacyOwnedSet = LegacyOwnedSet(0);

    const fn bit(row: LegacyOwned) -> u16 {
        1 << (row as u16)
    }

    fn insert(&mut self, row: LegacyOwned) {
        self.0 |= Self::bit(row);
    }

    /// Whether `row` was hit.
    pub const fn contains(self, row: LegacyOwned) -> bool {
        self.0 & Self::bit(row) != 0
    }

    /// Whether no row was hit.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

// ==================== the mapping tables ====================

/// One effect's legacy expression, for a domain whose legacy command **is** answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnsweredRow {
    /// The effect becomes `command`, whose terminal answer arrives as `reply`.
    Command { command: HostCommand, reply: LegacyReply },
    /// The legacy protocol cannot express the effect at all.
    Absent(LegacyOwned),
}

/// One effect's legacy expression, for a domain whose legacy command is **never** answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnansweredRow {
    /// The effect becomes `command`, which nothing replies to; `owned` names what stands in for the
    /// missing result.
    Command { command: HostCommand, owned: LegacyOwned },
    /// The legacy protocol cannot express the effect at all.
    Absent(LegacyOwned),
}

impl AnsweredRow {
    /// The legacy command this row emits, if any.
    pub const fn command(self) -> Option<HostCommand> {
        match self {
            AnsweredRow::Command { command, .. } => Some(command),
            AnsweredRow::Absent(_) => None,
        }
    }
}

impl UnansweredRow {
    /// The legacy command this row emits, if any.
    pub const fn command(self) -> Option<HostCommand> {
        match self {
            UnansweredRow::Command { command, .. } => Some(command),
            UnansweredRow::Absent(_) => None,
        }
    }
}

/// Where a catalog effect goes. The two removal rows are the sharpest edge of the whole migration:
/// the new store takes an object identity, the old repositories take a namespace.
pub const fn catalog_row(effect: CatalogEffect) -> UnansweredRow {
    match effect {
        CatalogEffect::ReadCatalog { .. } => UnansweredRow::Command {
            command: HostCommand::RescanStore { commits: 1 },
            owned: LegacyOwned::StoreRevision,
        },
        CatalogEffect::ReadTripMembers { .. } => UnansweredRow::Absent(LegacyOwned::TripCascade),
        CatalogEffect::RemoveObject { .. } => UnansweredRow::Absent(LegacyOwned::ObjectNamespace),
    }
}

/// Where a retention effect goes. The legacy command carries only the stamp; the retention *level*
/// beside it is the one already in the sidecar (retention re-reads it every time), so nothing is
/// lost by leaving it behind.
pub const fn retention_row(effect: RetentionEffect) -> UnansweredRow {
    match effect {
        RetentionEffect::WriteRouteMetadata { id, meta, .. } => UnansweredRow::Command {
            command: HostCommand::StampRouteUsed { id, utc: meta.last_used_utc },
            owned: LegacyOwned::SidecarAck,
        },
        RetentionEffect::WriteRideMetadata { id, synced_at, .. } => UnansweredRow::Command {
            command: HostCommand::StampRideSynced { id, utc: synced_at },
            owned: LegacyOwned::SidecarAck,
        },
    }
}

/// Where a recorder effect goes. Only the two ride *closes* have a legacy command; the journal is
/// the legacy host's own business.
pub const fn recorder_row(effect: RecorderEffect) -> UnansweredRow {
    match effect {
        RecorderEffect::Append { .. } | RecorderEffect::Checkpoint { .. } => {
            UnansweredRow::Absent(LegacyOwned::RecorderJournal)
        }
        RecorderEffect::Finalize { .. } => UnansweredRow::Command {
            command: HostCommand::FinishTrack(TrackAction::Save),
            owned: LegacyOwned::RideCloseAck,
        },
        RecorderEffect::Discard { .. } => UnansweredRow::Command {
            command: HostCommand::FinishTrack(TrackAction::Discard),
            owned: LegacyOwned::RideCloseAck,
        },
    }
}

/// Where the bond removal goes. A link fact confirms it; nothing answers it.
pub const fn bond_row(effect: BondEffect) -> UnansweredRow {
    match effect {
        BondEffect::Forget { .. } => {
            UnansweredRow::Command { command: HostCommand::ForgetBond, owned: LegacyOwned::BondAck }
        }
    }
}

/// Where a weather effect goes: nowhere. The legacy protocol never had a weather command.
pub const fn weather_row(effect: WeatherEffect) -> UnansweredRow {
    match effect {
        WeatherEffect::RequestRefresh { .. } | WeatherEffect::OpenInstalledData { .. } => {
            UnansweredRow::Absent(LegacyOwned::WeatherProtocol)
        }
    }
}

/// Where a navigator effect goes. One legacy plan command covers acquire, step and commit, so the
/// *acquire* carries the whole request and the rest of the sequence has no legacy expression.
pub const fn navigator_row(effect: NavigatorEffect) -> AnsweredRow {
    match effect {
        NavigatorEffect::Acquire { work: PlannerWork::Route(request), .. } => {
            AnsweredRow::Command { command: HostCommand::PlanRoute(request), reply: LegacyReply::RoutePlan }
        }
        NavigatorEffect::Acquire { work: PlannerWork::Detour(request), .. } => {
            AnsweredRow::Command { command: HostCommand::PlanDetour(request), reply: LegacyReply::DetourPlan }
        }
        NavigatorEffect::Step { .. } | NavigatorEffect::CommitRoute { .. } => {
            AnsweredRow::Absent(LegacyOwned::PlannerPacing)
        }
        NavigatorEffect::CommitDetour { .. } => {
            AnsweredRow::Command { command: HostCommand::CommitDetour, reply: LegacyReply::DetourCommit }
        }
        NavigatorEffect::Release { .. } => AnsweredRow::Absent(LegacyOwned::PlannerRelease),
    }
}

/// Where the settings write goes — the one effect whose legacy command matches it exactly.
pub const fn settings_row(effect: SettingsEffect) -> AnsweredRow {
    match effect {
        SettingsEffect::PersistRevision { revision, .. } => AnsweredRow::Command {
            command: HostCommand::PersistSettings { revision },
            reply: LegacyReply::SettingsWrite,
        },
    }
}

/// Where a DFU effect goes.
pub const fn dfu_row(effect: DfuEffect) -> AnsweredRow {
    match effect {
        DfuEffect::Scan { .. } => {
            AnsweredRow::Command { command: HostCommand::Dfu(DfuAction::Scan), reply: LegacyReply::DfuScan }
        }
        DfuEffect::ArmInstall { .. } => {
            AnsweredRow::Command { command: HostCommand::Dfu(DfuAction::Install), reply: LegacyReply::DfuInstall }
        }
    }
}

/// Where the free-space measurement goes.
pub const fn storage_info_row(effect: StorageInfoEffect) -> AnsweredRow {
    match effect {
        StorageInfoEffect::MeasureFreeSpace { .. } => {
            AnsweredRow::Command { command: HostCommand::ScanCardFree, reply: LegacyReply::CardScan }
        }
    }
}

/// The reply class a legacy event answers, or `None` when it is a fact rather than an answer to
/// anything. Exhaustive: a sixteenth [`HostEvent`] does not compile until it is placed.
pub const fn event_reply(event: &HostEvent) -> Option<LegacyReply> {
    match event {
        HostEvent::NavPlanned(_) => Some(LegacyReply::RoutePlan),
        HostEvent::DetourPlanned(_) => Some(LegacyReply::DetourPlan),
        HostEvent::DetourCommitted(_) => Some(LegacyReply::DetourCommit),
        HostEvent::SettingsPersisted { .. } | HostEvent::SettingsPersistFailed { .. } => {
            Some(LegacyReply::SettingsWrite)
        }
        HostEvent::DfuScanned(_) => Some(LegacyReply::DfuScan),
        HostEvent::DfuInstallBegan | HostEvent::DfuInstallFailed(_) => Some(LegacyReply::DfuInstall),
        HostEvent::CardScanned { .. } => Some(LegacyReply::CardScan),
        HostEvent::StoreChanged
        | HostEvent::RouteUploaded { .. }
        | HostEvent::TripUploaded { .. }
        | HostEvent::Warning(_)
        | HostEvent::UpdateConfirmed(_)
        | HostEvent::UpdateFailed { .. } => None,
    }
}

// ==================== the correlation store ====================

/// The operation token per in-flight legacy operation — the one field the old protocol had no room
/// for.
///
/// Deliberately **only** that. No retry counter, no cancellation flag, no replacement rule, no
/// terminal state: a domain owns those, and a token here is inert data handed straight back to the
/// domain that minted it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LegacyPending {
    route_plan: Option<OperationToken<NavigatorTag>>,
    detour_plan: Option<OperationToken<NavigatorTag>>,
    detour_commit: Option<OperationToken<NavigatorTag>>,
    settings_write: Option<OperationToken<SettingsTag>>,
    dfu_scan: Option<OperationToken<DfuTag>>,
    dfu_install: Option<OperationToken<DfuTag>>,
    card_scan: Option<OperationToken<StorageInfoTag>>,
}

impl LegacyPending {
    /// Nothing in flight.
    pub const fn new() -> Self {
        LegacyPending {
            route_plan: None,
            detour_plan: None,
            detour_commit: None,
            settings_write: None,
            dfu_scan: None,
            dfu_install: None,
            card_scan: None,
        }
    }

    /// Whether `class` has an operation still owed a reply.
    pub const fn holds(&self, class: LegacyReply) -> bool {
        match class {
            LegacyReply::RoutePlan => self.route_plan.is_some(),
            LegacyReply::DetourPlan => self.detour_plan.is_some(),
            LegacyReply::DetourCommit => self.detour_commit.is_some(),
            LegacyReply::SettingsWrite => self.settings_write.is_some(),
            LegacyReply::DfuScan => self.dfu_scan.is_some(),
            LegacyReply::DfuInstall => self.dfu_install.is_some(),
            LegacyReply::CardScan => self.card_scan.is_some(),
        }
    }

    /// Whether nothing at all is in flight.
    pub fn is_empty(&self) -> bool {
        LegacyReply::ALL.iter().all(|&class| !self.holds(class))
    }
}

/// Read the token a reply answers **without consuming it**. An event whose class has nothing in
/// flight is an explicit error: nobody asked for it, and inventing a token would forge an answer.
fn peek<Tag>(slot: &Option<OperationToken<Tag>>, class: LegacyReply) -> Result<OperationToken<Tag>, LegacyError> {
    slot.ok_or(LegacyError::NoPendingOperation(class))
}

/// Put a translated outcome into its domain slot and close the operation out — but **only** once
/// the outcome is actually in the slot.
///
/// The order matters: a refused delivery means the pass has not consumed the previous answer yet,
/// so the executor will offer this one again. Consuming the correlation slot first would make that
/// retry an unrequested event and lose the result for good.
fn settle<T, Tag>(
    slot: &mut Slot<T>,
    outcome: T,
    class: LegacyReply,
    pending: &mut Option<OperationToken<Tag>>,
) -> Result<(), LegacyError> {
    slot.try_put(outcome).map_err(|_| LegacyError::OutcomeSlotFull(class))?;
    *pending = None;
    Ok(())
}

// ==================== the adapter ====================

/// Why a translation could not be completed. Every variant is a *caller* fault the adapter refuses
/// to paper over — an unrequested reply, a second request for a class already in flight, or an
/// unconsumed slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyError {
    /// A legacy event arrived for a class with no operation in flight.
    NoPendingOperation(LegacyReply),
    /// The domain's outcome slot still holds an unconsumed answer.
    OutcomeSlotFull(LegacyReply),
    /// A fact could not be merged — the only case is a second unconsumed boot update result.
    Fact(FactMergeError),
}

/// What one [`effects_to_commands`](LegacyAdapter::effects_to_commands) call did.
///
/// The counters exist so nothing is silently skipped: every occupied slot is translated, put back
/// because the mailbox was full, put back because its reply class is busy, or left because the
/// legacy protocol cannot express it — and the four counts add up to the number of occupied slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LegacyReport {
    /// Effects that became legacy commands.
    pub translated: u8,
    /// Effects put back because the mailbox was full — backpressure, offered again next pass.
    pub deferred: u8,
    /// Effects put back because their reply class is still owed an answer.
    pub busy: u8,
    /// Effects left in their slot because the legacy protocol cannot express them.
    pub left: u8,
    /// Which [`LegacyOwned`] rows this pass hit.
    pub owned: LegacyOwnedSet,
}

impl LegacyReport {
    /// Note an effect with no legacy expression: it stays with its owner.
    fn leave(&mut self, owned: LegacyOwned) {
        self.owned.insert(owned);
        self.left += 1;
    }

    /// Push a translated command, or record the backpressure. `true` when it went out.
    fn send<const N: usize>(&mut self, command: HostCommand, mailbox: &mut HostMailbox<N>) -> bool {
        if mailbox.push_coalesced(command) {
            self.translated += 1;
            true
        } else {
            self.deferred += 1;
            false
        }
    }
}

/// Everything the adapter fills for the next pass: the domain outcome slots, the external facts,
/// and the keyed derived answers.
///
/// One value rather than three loose borrows, because a host builds all three between two passes
/// and hands them to [`PassInputs`](super::pass::PassInputs) together.
#[derive(Debug, PartialEq, Eq)]
pub struct LegacyInputs {
    /// Terminal answers, each carrying the token the adapter stored for it.
    pub outcomes: OutcomeSlots,
    /// Facts nobody asked for.
    pub facts: ExternalFacts,
    /// Keyed answers to the two derived levels.
    pub derived: DerivedInputs,
}

impl LegacyInputs {
    /// Nothing to report.
    pub fn new() -> Self {
        LegacyInputs { outcomes: OutcomeSlots::new(), facts: ExternalFacts::NONE, derived: DerivedInputs::NONE }
    }
}

impl Default for LegacyInputs {
    fn default() -> Self {
        LegacyInputs::new()
    }
}

/// The identity the adapter files legacy store commits under. The legacy protocol has exactly one
/// store and never names it, so a fixed value is the honest translation of "the store".
const LEGACY_STORE: StoreIdentity = StoreIdentity::new(1);

/// The one temporary translator between DeviceCore's slots and the legacy host protocol.
///
/// Its whole state is [`LegacyPending`] plus the synthesised store revision
/// ([`LegacyOwned::StoreRevision`]). See the module documentation for what that deliberately
/// excludes.
#[derive(Debug, PartialEq, Eq)]
pub struct LegacyAdapter {
    pending: LegacyPending,
    /// How many store commits the legacy protocol has reported. Stands in for the revision the old
    /// `StoreChanged` never carried, so the pass still sees one *edge* per commit.
    commits: u64,
}

impl LegacyAdapter {
    /// A fresh adapter with nothing in flight.
    pub const fn new() -> Self {
        LegacyAdapter { pending: LegacyPending::new(), commits: 0 }
    }

    /// What is in flight — the correlation state, for inspection and tests.
    pub fn pending(&self) -> &LegacyPending {
        &self.pending
    }

    /// Translate this pass's effects into legacy commands.
    ///
    /// Each domain slot is taken, mapped through its table, and either pushed to `mailbox` or **put
    /// back into `effects` unchanged** — a full mailbox, a busy reply class and a row the legacy
    /// protocol cannot express all leave the effect for the caller to offer on a later call. That is
    /// what makes staging one plan across several calls lossless; it does not make the *domain* try
    /// again (see the module docs on the in-flight latch).
    pub fn effects_to_commands<const N: usize>(
        &mut self,
        effects: &mut EffectSlots,
        mailbox: &mut HostMailbox<N>,
    ) -> LegacyReport {
        let mut report = LegacyReport::default();

        /// A domain the legacy protocol never answers: no token is stored, so the effect is simply
        /// sent or kept.
        macro_rules! unanswered {
            ($field:ident, $row:path) => {
                if let Some(effect) = effects.$field.take() {
                    let keep = match $row(effect) {
                        UnansweredRow::Absent(owned) => {
                            report.leave(owned);
                            true
                        }
                        UnansweredRow::Command { command, owned } => {
                            // The row is recorded only on a command that actually went out: a
                            // deferred effect has not hit the row yet, it was never sent.
                            if report.send(command, mailbox) {
                                report.owned.insert(owned);
                                false
                            } else {
                                true
                            }
                        }
                    };
                    if keep {
                        let _ = effects.$field.try_put(effect);
                    }
                }
            };
        }

        /// A domain whose command is answered: the effect's token is stored under the reply class
        /// `$slot` selects, and only once the command has actually gone out.
        macro_rules! answered {
            ($field:ident, $row:path, $reply:ident => $slot:expr) => {
                if let Some(effect) = effects.$field.take() {
                    let keep = match $row(effect) {
                        AnsweredRow::Absent(owned) => {
                            report.leave(owned);
                            true
                        }
                        AnsweredRow::Command { command, reply } => {
                            let $reply = reply;
                            let slot = $slot;
                            if slot.is_some() {
                                // Replacing a token still owed a reply would leave the first
                                // operation permanently unanswerable.
                                report.busy += 1;
                                true
                            } else if report.send(command, mailbox) {
                                *slot = Some(effect.token());
                                false
                            } else {
                                true
                            }
                        }
                    };
                    if keep {
                        let _ = effects.$field.try_put(effect);
                    }
                }
            };
        }

        unanswered!(catalog, catalog_row);
        unanswered!(retention, retention_row);
        unanswered!(recorder, recorder_row);
        // Every class named explicitly: this is the one module whose whole job is correlation, so a
        // future table edit must not be able to file a reply under the wrong slot by falling into a
        // catch-all. `navigator_row` names exactly these three, and the tables are exhaustive.
        answered!(navigator, navigator_row, class => match class {
            LegacyReply::RoutePlan => &mut self.pending.route_plan,
            LegacyReply::DetourPlan => &mut self.pending.detour_plan,
            LegacyReply::DetourCommit => &mut self.pending.detour_commit,
            _ => unreachable!("navigator_row names only navigation reply classes"),
        });
        answered!(settings, settings_row, _class => &mut self.pending.settings_write);
        unanswered!(weather, weather_row);
        answered!(dfu, dfu_row, class => match class {
            LegacyReply::DfuScan => &mut self.pending.dfu_scan,
            LegacyReply::DfuInstall => &mut self.pending.dfu_install,
            _ => unreachable!("dfu_row names only the two DFU reply classes"),
        });
        unanswered!(bond, bond_row);
        answered!(storage_info, storage_info_row, _class => &mut self.pending.card_scan);

        report
    }

    /// Re-emit the two derived levels as their legacy command cues.
    ///
    /// A level, not an operation: `&self` because there is nothing to remember, which is the
    /// structural form of "derived levels do not receive operation tokens". A cue that does not fit
    /// the mailbox is simply seen again next pass — the legacy mailbox coalesces both classes for
    /// exactly this reason.
    pub fn needs_to_commands<const N: usize>(&self, needs: &DerivedNeeds, mailbox: &mut HostMailbox<N>) {
        if let Some(key) = needs.ride_track {
            let _ = mailbox.push_coalesced(HostCommand::LoadRideTrack { id: key.ride });
        }
        if needs.nav_preview.is_some() {
            let _ = mailbox.push_coalesced(HostCommand::RefreshNavPreview);
        }
    }

    /// Translate one legacy event into the next pass's inputs.
    ///
    /// A terminal answer becomes its domain's typed outcome carrying the **original** token; every
    /// other event becomes a named external fact. Whether a returned token still counts is the
    /// domain's decision, made inside the pass — the adapter neither knows nor asks.
    pub fn event_to_inputs(&mut self, event: HostEvent, next: &mut LegacyInputs) -> Result<(), LegacyError> {
        let pending = &mut self.pending;
        match event {
            // ---- terminal answers: the stored token comes back with the result ----
            HostEvent::NavPlanned(result) => {
                let token = peek(&pending.route_plan, LegacyReply::RoutePlan)?;
                let outcome = match result {
                    Ok(route) => NavigatorOutcome::PlanFinished { token, route },
                    Err(error) => NavigatorOutcome::Failed { token, error: NavigatorError::Plan(error) },
                };
                settle(&mut next.outcomes.navigator, outcome, LegacyReply::RoutePlan, &mut pending.route_plan)
            }
            HostEvent::DetourPlanned(result) => {
                let token = peek(&pending.detour_plan, LegacyReply::DetourPlan)?;
                let outcome = match result {
                    Ok(preview) => NavigatorOutcome::DetourFinished { token, preview },
                    Err(error) => NavigatorOutcome::Failed { token, error: NavigatorError::Plan(error) },
                };
                settle(&mut next.outcomes.navigator, outcome, LegacyReply::DetourPlan, &mut pending.detour_plan)
            }
            HostEvent::DetourCommitted(result) => {
                let token = peek(&pending.detour_commit, LegacyReply::DetourCommit)?;
                let outcome = match result {
                    Ok(route) => NavigatorOutcome::DetourCommitted { token, route },
                    Err(error) => NavigatorOutcome::Failed { token, error: NavigatorError::Plan(error) },
                };
                settle(&mut next.outcomes.navigator, outcome, LegacyReply::DetourCommit, &mut pending.detour_commit)
            }
            HostEvent::SettingsPersisted { revision } => {
                let token = peek(&pending.settings_write, LegacyReply::SettingsWrite)?;
                let outcome = SettingsOutcome::Persisted { token, revision };
                settle(&mut next.outcomes.settings, outcome, LegacyReply::SettingsWrite, &mut pending.settings_write)
            }
            HostEvent::SettingsPersistFailed { revision, error } => {
                let token = peek(&pending.settings_write, LegacyReply::SettingsWrite)?;
                let outcome = SettingsOutcome::PersistFailed { token, revision, error };
                settle(&mut next.outcomes.settings, outcome, LegacyReply::SettingsWrite, &mut pending.settings_write)
            }
            HostEvent::DfuScanned(result) => {
                let token = peek(&pending.dfu_scan, LegacyReply::DfuScan)?;
                let outcome = match result {
                    Ok(report) => DfuOutcome::ScanFinished { token, report },
                    Err(error) => DfuOutcome::ScanFailed { token, error },
                };
                settle(&mut next.outcomes.dfu, outcome, LegacyReply::DfuScan, &mut pending.dfu_scan)
            }
            HostEvent::DfuInstallBegan => {
                let token = peek(&pending.dfu_install, LegacyReply::DfuInstall)?;
                let outcome = DfuOutcome::InstallBegan { token };
                settle(&mut next.outcomes.dfu, outcome, LegacyReply::DfuInstall, &mut pending.dfu_install)
            }
            HostEvent::DfuInstallFailed(error) => {
                let token = peek(&pending.dfu_install, LegacyReply::DfuInstall)?;
                let outcome = DfuOutcome::InstallFailed { token, error };
                settle(&mut next.outcomes.dfu, outcome, LegacyReply::DfuInstall, &mut pending.dfu_install)
            }
            // The legacy `None` folded "no medium" and "the scan failed" into one value. The domain
            // separates them, and the honest translation of an unqualified `None` is the failed scan
            // — "not mounted" is a claim the legacy event never made.
            HostEvent::CardScanned { free_bytes } => {
                let token = peek(&pending.card_scan, LegacyReply::CardScan)?;
                let outcome = match free_bytes {
                    Some(free_bytes) => StorageInfoOutcome::Measured { token, free_bytes },
                    None => StorageInfoOutcome::Failed { token, error: StorageInfoError::ScanFailed },
                };
                settle(&mut next.outcomes.storage_info, outcome, LegacyReply::CardScan, &mut pending.card_scan)
            }

            // ---- facts: nobody asked, so nothing carries a token ----
            HostEvent::StoreChanged => {
                self.commits += 1;
                next.facts
                    .note_store_revision(StoreRevision { store: LEGACY_STORE, revision: Revision::new(self.commits) });
                Ok(())
            }
            HostEvent::RouteUploaded { id, replaced, elevation } => {
                next.facts.note_route_upload(RouteUpload { id, replaced, elevation });
                Ok(())
            }
            HostEvent::TripUploaded { id, replaced } => {
                next.facts.note_trip_upload(TripUpload { id, replaced });
                Ok(())
            }
            HostEvent::Warning(flags) => {
                next.facts.raise_warnings(flags);
                Ok(())
            }
            HostEvent::UpdateConfirmed(version) => {
                next.facts.note_update_result(UpdateResult::Confirmed(version)).map_err(LegacyError::Fact)
            }
            HostEvent::UpdateFailed { why, staged } => {
                next.facts.note_update_result(UpdateResult::Failed { why, staged }).map_err(LegacyError::Fact)
            }
        }
    }

    /// Attach the ride-track key to a legacy bulk fill.
    ///
    /// The legacy feeders (`set_ride_profile` + `set_ride_preview`) say *what* was read and never
    /// *about which subject*, which is exactly why a delayed fill can land on the wrong ride. The
    /// key the executor started the read with closes that: DeviceCore compares it against the need
    /// it currently holds and drops an answer about something else.
    pub fn feed_ride_track(&self, key: RideTrackKey, result: DerivedResult, next: &mut LegacyInputs) {
        next.derived.ride_track = Some(DerivedInput { key, result });
    }

    /// Attach the nav-preview key to a legacy bulk fill — the twin of
    /// [`feed_ride_track`](Self::feed_ride_track), separately named for the same reason
    /// [`DerivedTargets`](super::derived::DerivedTargets) names its two fields: they are the same
    /// type, and swapping them would draw a ride's track over a route overview.
    pub fn feed_nav_preview(&self, key: NavPreviewKey, result: DerivedResult, next: &mut LegacyInputs) {
        next.derived.nav_preview = Some(DerivedInput { key, result });
    }
}

impl Default for LegacyAdapter {
    fn default() -> Self {
        LegacyAdapter::new()
    }
}

// Layout tripwires. The adapter is a handful of tokens and a counter — a growth here means product
// state moved into the translator, which is the one thing it must never hold.
const _: () = assert!(core::mem::size_of::<LegacyPending>() <= 56, "seven optional tokens");
const _: () = assert!(core::mem::size_of::<LegacyAdapter>() <= 64, "seven tokens and a commit counter");
const _: () = assert!(core::mem::size_of::<LegacyReport>() <= 8, "four counters and a bit set");
// A row is one legacy command plus a tag, so it is pinned to `HostCommand`'s own budget rather than
// to a number of its own: the row must not become somewhere a payload can hide beside the command.
const ROW_BUDGET: usize = core::mem::size_of::<HostCommand>() + 8;
const _: () = assert!(core::mem::size_of::<AnsweredRow>() <= ROW_BUDGET, "one bounded command plus a tag");
const _: () = assert!(core::mem::size_of::<UnansweredRow>() <= ROW_BUDGET, "one bounded command plus a tag");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_core::migration::{command_migration, event_migration, LegacyOwner, LegacyRole, LEGACY_EVENTS};
    use crate::device_core::{BondTag, CatalogTag, DataIdentity, RecorderTag, RetentionTag, TokenSource, WeatherTag};
    use crate::dfu::{clamp, DfuFailure, DfuInstallError, DfuScanError, DfuScanReport};
    use crate::host::DetourPreview;
    use crate::retention::{Retention, RouteRetentionMeta};
    use crate::screen::WarningFlags;
    use crate::{DetourRequest, NavRequest};
    use obc_ports::SettingsSaveError;
    use obc_route::nav::NavError;

    /// A default-sized mailbox. Named, because `HostMailbox`'s default capacity is a type default
    /// and inference does not apply one.
    fn mailbox() -> HostMailbox {
        HostMailbox::new()
    }

    fn drain(mailbox: &mut HostMailbox) -> heapless::Vec<HostCommand, 18> {
        let mut out = heapless::Vec::new();
        while let Some(cmd) = mailbox.pop() {
            out.push(cmd).unwrap();
        }
        out
    }

    /// One token source per domain, so every effect in a test carries a distinguishable live token.
    struct Ops {
        catalog: TokenSource<CatalogTag>,
        retention: TokenSource<RetentionTag>,
        recorder: TokenSource<RecorderTag>,
        navigator: TokenSource<NavigatorTag>,
        settings: TokenSource<SettingsTag>,
        weather: TokenSource<WeatherTag>,
        dfu: TokenSource<DfuTag>,
        bond: TokenSource<BondTag>,
        storage: TokenSource<StorageInfoTag>,
    }

    impl Ops {
        fn new() -> Self {
            Ops {
                catalog: TokenSource::new(),
                retention: TokenSource::new(),
                recorder: TokenSource::new(),
                navigator: TokenSource::new(),
                settings: TokenSource::new(),
                weather: TokenSource::new(),
                dfu: TokenSource::new(),
                bond: TokenSource::new(),
                storage: TokenSource::new(),
            }
        }
    }

    fn route_request() -> NavRequest {
        NavRequest::new((0, 0), (1, 1), "goal")
    }

    fn detour_request() -> DetourRequest {
        DetourRequest { route: 0, from: (0, 0), progress_m: 100, target_m: 500 }
    }

    fn meta() -> RouteRetentionMeta {
        RouteRetentionMeta::new(Retention::Week1, 1_700_000_000)
    }

    // ==================== the mapping tables ====================

    /// Every effect the nine domains can emit, with the exact legacy row it maps to. This *is* the
    /// mapping table: a new effect variant fails its domain's exhaustive `const fn`, and a changed
    /// mapping fails here.
    #[test]
    fn every_effect_has_an_exact_legacy_row() {
        let mut ops = Ops::new();

        assert_eq!(
            catalog_row(CatalogEffect::ReadCatalog { token: ops.catalog.issue() }),
            UnansweredRow::Command {
                command: HostCommand::RescanStore { commits: 1 },
                owned: LegacyOwned::StoreRevision
            }
        );
        assert_eq!(
            catalog_row(CatalogEffect::ReadTripMembers { token: ops.catalog.issue(), trip: 3 }),
            UnansweredRow::Absent(LegacyOwned::TripCascade)
        );
        assert_eq!(
            catalog_row(CatalogEffect::RemoveObject { token: ops.catalog.issue(), object: 3 }),
            UnansweredRow::Absent(LegacyOwned::ObjectNamespace)
        );

        assert_eq!(
            retention_row(RetentionEffect::WriteRouteMetadata { token: ops.retention.issue(), id: 4, meta: meta() }),
            UnansweredRow::Command {
                command: HostCommand::StampRouteUsed { id: 4, utc: 1_700_000_000 },
                owned: LegacyOwned::SidecarAck,
            }
        );
        assert_eq!(
            retention_row(RetentionEffect::WriteRideMetadata { token: ops.retention.issue(), id: 5, synced_at: 99 }),
            UnansweredRow::Command {
                command: HostCommand::StampRideSynced { id: 5, utc: 99 },
                owned: LegacyOwned::SidecarAck,
            }
        );

        assert_eq!(
            recorder_row(RecorderEffect::Append { token: ops.recorder.issue(), samples: 8 }),
            UnansweredRow::Absent(LegacyOwned::RecorderJournal)
        );
        assert_eq!(
            recorder_row(RecorderEffect::Checkpoint { token: ops.recorder.issue() }),
            UnansweredRow::Absent(LegacyOwned::RecorderJournal)
        );
        assert_eq!(
            recorder_row(RecorderEffect::Finalize { token: ops.recorder.issue() }),
            UnansweredRow::Command {
                command: HostCommand::FinishTrack(TrackAction::Save),
                owned: LegacyOwned::RideCloseAck,
            }
        );
        assert_eq!(
            recorder_row(RecorderEffect::Discard { token: ops.recorder.issue() }),
            UnansweredRow::Command {
                command: HostCommand::FinishTrack(TrackAction::Discard),
                owned: LegacyOwned::RideCloseAck,
            }
        );

        let token = ops.navigator.issue();
        assert_eq!(
            navigator_row(NavigatorEffect::Acquire { token, work: PlannerWork::Route(route_request()) }),
            AnsweredRow::Command { command: HostCommand::PlanRoute(route_request()), reply: LegacyReply::RoutePlan }
        );
        assert_eq!(
            navigator_row(NavigatorEffect::Acquire { token, work: PlannerWork::Detour(detour_request()) }),
            AnsweredRow::Command { command: HostCommand::PlanDetour(detour_request()), reply: LegacyReply::DetourPlan }
        );
        assert_eq!(navigator_row(NavigatorEffect::Step { token }), AnsweredRow::Absent(LegacyOwned::PlannerPacing));
        assert_eq!(
            navigator_row(NavigatorEffect::CommitRoute { token }),
            AnsweredRow::Absent(LegacyOwned::PlannerPacing)
        );
        assert_eq!(
            navigator_row(NavigatorEffect::CommitDetour { token }),
            AnsweredRow::Command { command: HostCommand::CommitDetour, reply: LegacyReply::DetourCommit }
        );
        assert_eq!(navigator_row(NavigatorEffect::Release { token }), AnsweredRow::Absent(LegacyOwned::PlannerRelease));

        assert_eq!(
            settings_row(SettingsEffect::PersistRevision { token: ops.settings.issue(), revision: 7 }),
            AnsweredRow::Command {
                command: HostCommand::PersistSettings { revision: 7 },
                reply: LegacyReply::SettingsWrite,
            }
        );

        assert_eq!(
            weather_row(WeatherEffect::RequestRefresh { token: ops.weather.issue() }),
            UnansweredRow::Absent(LegacyOwned::WeatherProtocol)
        );
        assert_eq!(
            weather_row(WeatherEffect::OpenInstalledData { token: ops.weather.issue(), data: DataIdentity::new(1) }),
            UnansweredRow::Absent(LegacyOwned::WeatherProtocol)
        );

        assert_eq!(
            dfu_row(DfuEffect::Scan { token: ops.dfu.issue() }),
            AnsweredRow::Command { command: HostCommand::Dfu(DfuAction::Scan), reply: LegacyReply::DfuScan }
        );
        assert_eq!(
            dfu_row(DfuEffect::ArmInstall { token: ops.dfu.issue() }),
            AnsweredRow::Command { command: HostCommand::Dfu(DfuAction::Install), reply: LegacyReply::DfuInstall }
        );

        assert_eq!(
            bond_row(BondEffect::Forget { token: ops.bond.issue() }),
            UnansweredRow::Command { command: HostCommand::ForgetBond, owned: LegacyOwned::BondAck }
        );
        assert_eq!(
            storage_info_row(StorageInfoEffect::MeasureFreeSpace { token: ops.storage.issue() }),
            AnsweredRow::Command { command: HostCommand::ScanCardFree, reply: LegacyReply::CardScan }
        );
    }

    /// Every legacy event's row: which reply class it answers, or that it is a fact.
    #[test]
    fn every_event_has_an_exact_legacy_row() {
        let rows: [(HostEvent, Option<LegacyReply>); LEGACY_EVENTS] = [
            (HostEvent::StoreChanged, None),
            (HostEvent::RouteUploaded { id: 1, replaced: false, elevation: None }, None),
            (HostEvent::TripUploaded { id: 2, replaced: true }, None),
            (HostEvent::Warning(WarningFlags::NO_GPS), None),
            (HostEvent::UpdateConfirmed(clamp("v2")), None),
            (HostEvent::UpdateFailed { why: DfuFailure::Reverted, staged: None }, None),
            (HostEvent::NavPlanned(Ok(9)), Some(LegacyReply::RoutePlan)),
            (HostEvent::DetourPlanned(Err(NavError::NoPath)), Some(LegacyReply::DetourPlan)),
            (HostEvent::DetourCommitted(Ok(10)), Some(LegacyReply::DetourCommit)),
            (HostEvent::SettingsPersisted { revision: 3 }, Some(LegacyReply::SettingsWrite)),
            (
                HostEvent::SettingsPersistFailed { revision: 3, error: SettingsSaveError::Backend },
                Some(LegacyReply::SettingsWrite),
            ),
            (HostEvent::DfuScanned(Err(DfuScanError::NotFound)), Some(LegacyReply::DfuScan)),
            (HostEvent::DfuInstallBegan, Some(LegacyReply::DfuInstall)),
            (HostEvent::DfuInstallFailed(DfuInstallError::NoCard), Some(LegacyReply::DfuInstall)),
            (HostEvent::CardScanned { free_bytes: Some(1) }, Some(LegacyReply::CardScan)),
        ];
        for (event, want) in &rows {
            assert_eq!(event_reply(event), *want, "{event:?}");
        }
        // Every reply class is reachable from a real event — a class nothing answers would be a
        // correlation slot that fills and never drains.
        for class in LegacyReply::ALL {
            assert!(rows.iter().any(|(_, reply)| *reply == Some(class)), "{class:?} has no answering event");
        }
    }

    /// The adapter's tables agree with DC3's inventory: every command it emits lands on the owner
    /// #1433 Appendix A assigned it, every event it turns into an outcome is an `Outcome` row, and
    /// every event it turns into a fact is an `ExternalFact` row.
    ///
    /// Two independent statements of the same mapping, cross-checked — which is what makes either
    /// of them worth having.
    #[test]
    fn every_mapping_agrees_with_the_dc3_inventory() {
        let mut ops = Ops::new();
        let token = ops.navigator.issue();
        let unanswered = [
            (catalog_row(CatalogEffect::ReadCatalog { token: ops.catalog.issue() }), LegacyOwner::Catalog),
            (
                retention_row(RetentionEffect::WriteRouteMetadata {
                    token: ops.retention.issue(),
                    id: 1,
                    meta: meta(),
                }),
                LegacyOwner::Retention,
            ),
            (
                retention_row(RetentionEffect::WriteRideMetadata { token: ops.retention.issue(), id: 2, synced_at: 5 }),
                LegacyOwner::Retention,
            ),
            (recorder_row(RecorderEffect::Finalize { token: ops.recorder.issue() }), LegacyOwner::Recorder),
            (recorder_row(RecorderEffect::Discard { token: ops.recorder.issue() }), LegacyOwner::Recorder),
            (bond_row(BondEffect::Forget { token: ops.bond.issue() }), LegacyOwner::Bond),
        ];
        for (row, owner) in unanswered {
            let command = row.command().expect("this row emits a command");
            assert_eq!(command_migration(&command).owner, owner, "{command:?} lands on a different owner in DC3");
        }

        let answered = [
            (
                navigator_row(NavigatorEffect::Acquire { token, work: PlannerWork::Route(route_request()) }),
                LegacyOwner::Navigator,
            ),
            (
                navigator_row(NavigatorEffect::Acquire { token, work: PlannerWork::Detour(detour_request()) }),
                LegacyOwner::Navigator,
            ),
            (navigator_row(NavigatorEffect::CommitDetour { token }), LegacyOwner::Navigator),
            (
                settings_row(SettingsEffect::PersistRevision { token: ops.settings.issue(), revision: 1 }),
                LegacyOwner::Settings,
            ),
            (dfu_row(DfuEffect::Scan { token: ops.dfu.issue() }), LegacyOwner::Dfu),
            (dfu_row(DfuEffect::ArmInstall { token: ops.dfu.issue() }), LegacyOwner::Dfu),
            (
                storage_info_row(StorageInfoEffect::MeasureFreeSpace { token: ops.storage.issue() }),
                LegacyOwner::StorageInfo,
            ),
        ];
        for (row, owner) in answered {
            let command = row.command().expect("this row emits a command");
            assert_eq!(command_migration(&command).owner, owner, "{command:?} lands on a different owner in DC3");
        }

        // The two derived levels are DC3's `DerivedNeed` rows — no token, by classification.
        for command in [HostCommand::LoadRideTrack { id: 1 }, HostCommand::RefreshNavPreview] {
            let row = command_migration(&command);
            assert_eq!(row.role, LegacyRole::DerivedNeed);
            assert_eq!(row.owner, LegacyOwner::Derived);
        }

        // The event half: an answer is an `Outcome` row, a fact is an `ExternalFact` row.
        for event in [
            HostEvent::StoreChanged,
            HostEvent::RouteUploaded { id: 1, replaced: false, elevation: None },
            HostEvent::TripUploaded { id: 2, replaced: false },
            HostEvent::Warning(WarningFlags::NO_GPS),
            HostEvent::UpdateConfirmed(clamp("v2")),
            HostEvent::UpdateFailed { why: DfuFailure::Reverted, staged: None },
            HostEvent::NavPlanned(Ok(3)),
            HostEvent::DetourPlanned(Err(NavError::NoPath)),
            HostEvent::DetourCommitted(Ok(4)),
            HostEvent::SettingsPersisted { revision: 1 },
            HostEvent::SettingsPersistFailed { revision: 1, error: SettingsSaveError::Backend },
            HostEvent::DfuScanned(Err(DfuScanError::NotFound)),
            HostEvent::DfuInstallBegan,
            HostEvent::DfuInstallFailed(DfuInstallError::NoCard),
            HostEvent::CardScanned { free_bytes: None },
        ] {
            let want = if event_reply(&event).is_some() { LegacyRole::Outcome } else { LegacyRole::ExternalFact };
            assert_eq!(event_migration(&event).role, want, "{event:?}");
        }
    }

    /// Every `LegacyOwned` row names the slice that deletes it. A row without one is a permanent
    /// exception dressed up as a migration step.
    #[test]
    fn every_owned_row_names_its_deleting_slice() {
        for row in LegacyOwned::ALL {
            assert!(row.deletes_in().starts_with('#'), "{row:?} must name an issue");
        }
        // The bit set has to be able to hold every row — a row past bit 15 would silently alias.
        assert!(LegacyOwned::ALL.len() <= 16, "the report's bit set is a u16");
        let mut set = LegacyOwnedSet::NONE;
        assert!(set.is_empty());
        for row in LegacyOwned::ALL {
            set.insert(row);
        }
        for row in LegacyOwned::ALL {
            assert!(set.contains(row), "{row:?} is not distinguishable in the report");
        }
    }

    // ==================== translating a pass's effects ====================

    /// A full effect batch: the nine slots go out as the commands the tables name, and the effects
    /// the legacy protocol cannot express are **left in their slots** rather than dropped — the same
    /// rule the pass applies to an outcome with no owner.
    #[test]
    fn a_full_effect_batch_translates_and_leaves_nothing_behind() {
        let mut ops = Ops::new();
        let mut adapter = LegacyAdapter::new();
        let mut mail = mailbox();
        let mut effects = EffectSlots::new();

        let removal = CatalogEffect::RemoveObject { token: ops.catalog.issue(), object: 7 };
        effects.catalog.try_put(removal).unwrap();
        effects
            .retention
            .try_put(RetentionEffect::WriteRouteMetadata { token: ops.retention.issue(), id: 4, meta: meta() })
            .unwrap();
        effects.recorder.try_put(RecorderEffect::Finalize { token: ops.recorder.issue() }).unwrap();
        let work = PlannerWork::Route(route_request());
        effects.navigator.try_put(NavigatorEffect::Acquire { token: ops.navigator.issue(), work }).unwrap();
        effects.settings.try_put(SettingsEffect::PersistRevision { token: ops.settings.issue(), revision: 2 }).unwrap();
        let refresh = WeatherEffect::RequestRefresh { token: ops.weather.issue() };
        effects.weather.try_put(refresh).unwrap();
        effects.dfu.try_put(DfuEffect::Scan { token: ops.dfu.issue() }).unwrap();
        effects.bond.try_put(BondEffect::Forget { token: ops.bond.issue() }).unwrap();
        effects.storage_info.try_put(StorageInfoEffect::MeasureFreeSpace { token: ops.storage.issue() }).unwrap();

        let report = adapter.effects_to_commands(&mut effects, &mut mail);
        assert_eq!(report.translated, 7, "seven effects have a legacy command");
        assert_eq!(report.left, 2, "the removal and the weather refresh have none");
        assert_eq!(report.busy + report.deferred, 0);
        assert!(report.owned.contains(LegacyOwned::ObjectNamespace));
        assert!(report.owned.contains(LegacyOwned::WeatherProtocol));
        assert!(report.owned.contains(LegacyOwned::SidecarAck) && report.owned.contains(LegacyOwned::BondAck));

        assert_eq!(effects.catalog.take(), Some(removal), "an untranslatable effect stays with its owner");
        assert_eq!(effects.weather.take(), Some(refresh));
        assert!(!effects.has_pending(), "everything else was consumed exactly once");

        let sent = drain(&mut mail);
        assert!(sent.contains(&HostCommand::StampRouteUsed { id: 4, utc: 1_700_000_000 }));
        assert!(sent.contains(&HostCommand::FinishTrack(TrackAction::Save)));
        assert!(sent.contains(&HostCommand::PlanRoute(route_request())));
        assert!(sent.contains(&HostCommand::PersistSettings { revision: 2 }));
        assert!(sent.contains(&HostCommand::Dfu(DfuAction::Scan)));
        assert!(sent.contains(&HostCommand::ForgetBond));
        assert!(sent.contains(&HostCommand::ScanCardFree));
        assert_eq!(sent.len(), 7);

        // Four reply classes are now owed an answer; the three unanswered commands armed nothing.
        for class in [LegacyReply::RoutePlan, LegacyReply::SettingsWrite, LegacyReply::DfuScan, LegacyReply::CardScan] {
            assert!(adapter.pending().holds(class), "{class:?} is owed a reply");
        }
        assert!(!adapter.pending().holds(LegacyReply::DetourPlan));
    }

    /// A second effect for a class that is still owed a reply is refused, and the **first** token
    /// stands. Replacing it would leave the first operation permanently unanswerable: the domain
    /// would wait for a result the adapter could no longer address to it.
    #[test]
    fn a_second_effect_cannot_replace_an_in_flight_token() {
        let mut ops = Ops::new();
        let mut adapter = LegacyAdapter::new();
        let mut mail = mailbox();

        let first = ops.settings.issue();
        let mut effects = EffectSlots::new();
        effects.settings.try_put(SettingsEffect::PersistRevision { token: first, revision: 1 }).unwrap();
        assert_eq!(adapter.effects_to_commands(&mut effects, &mut mail).translated, 1);

        let second = SettingsEffect::PersistRevision { token: ops.settings.issue(), revision: 2 };
        let mut effects = EffectSlots::new();
        effects.settings.try_put(second).unwrap();
        let report = adapter.effects_to_commands(&mut effects, &mut mail);
        assert_eq!(report.busy, 1, "the class is still in flight");
        assert_eq!(effects.settings.take(), Some(second), "the refused effect goes back to its owner unchanged");

        // The reply still addresses the *first* operation.
        let mut next = LegacyInputs::new();
        adapter.event_to_inputs(HostEvent::SettingsPersisted { revision: 1 }, &mut next).unwrap();
        assert_eq!(
            next.outcomes.settings.take(),
            Some(SettingsOutcome::Persisted { token: first, revision: 1 }),
            "the stored token is the one that went out"
        );
    }

    /// A full mailbox is backpressure, not a failure: the effect goes back into its slot, no reply
    /// is recorded as owed for an operation that never started, and the same effect goes out
    /// unchanged on the next pass.
    #[test]
    fn a_full_mailbox_puts_the_effect_back_and_arms_nothing() {
        let mut ops = Ops::new();
        let mut adapter = LegacyAdapter::new();
        let mut mail: HostMailbox<1> = HostMailbox::new();
        assert!(mail.push_coalesced(HostCommand::CancelDetour));

        let effect = StorageInfoEffect::MeasureFreeSpace { token: ops.storage.issue() };
        let mut effects = EffectSlots::new();
        effects.storage_info.try_put(effect).unwrap();
        let report = adapter.effects_to_commands(&mut effects, &mut mail);
        assert_eq!(report.deferred, 1);
        assert!(!effects.storage_info.is_empty(), "the effect is still in the caller's slots");
        assert!(!adapter.pending().holds(LegacyReply::CardScan), "nothing went out, so nothing is owed");

        // Room appears, and the **same** slots are offered again — the property the put-back
        // actually has, and the one the board's multi-step staging depends on.
        assert_eq!(mail.pop(), Some(HostCommand::CancelDetour));
        assert_eq!(adapter.effects_to_commands(&mut effects, &mut mail).translated, 1);
        assert!(effects.storage_info.is_empty(), "and now it is consumed exactly once");
        assert_eq!(mail.pop(), Some(HostCommand::ScanCardFree), "unchanged by the busy call");
        assert!(adapter.pending().holds(LegacyReply::CardScan));
    }

    // ==================== translating a legacy event ====================

    /// Every terminal event comes back carrying the token its effect went out with — the whole point
    /// of the correlation slot, across all seven reply classes.
    #[test]
    fn a_terminal_event_returns_the_original_token() {
        let mut ops = Ops::new();
        let mut adapter = LegacyAdapter::new();
        let mut mail = mailbox();

        let plan = ops.navigator.issue();
        let detour = ops.navigator.issue();
        let commit = ops.navigator.issue();
        let settings = ops.settings.issue();
        let scan = ops.dfu.issue();
        let install = ops.dfu.issue();
        let card = ops.storage.issue();

        // One navigator effect fits per pass by construction, so each goes out in its own.
        let mut send = |adapter: &mut LegacyAdapter, effect: NavigatorEffect| {
            let mut effects = EffectSlots::new();
            effects.navigator.try_put(effect).unwrap();
            assert_eq!(adapter.effects_to_commands(&mut effects, &mut mail).translated, 1);
        };
        send(&mut adapter, NavigatorEffect::Acquire { token: plan, work: PlannerWork::Route(route_request()) });
        send(&mut adapter, NavigatorEffect::Acquire { token: detour, work: PlannerWork::Detour(detour_request()) });
        send(&mut adapter, NavigatorEffect::CommitDetour { token: commit });

        let mut effects = EffectSlots::new();
        effects.settings.try_put(SettingsEffect::PersistRevision { token: settings, revision: 5 }).unwrap();
        effects.dfu.try_put(DfuEffect::Scan { token: scan }).unwrap();
        effects.storage_info.try_put(StorageInfoEffect::MeasureFreeSpace { token: card }).unwrap();
        assert_eq!(adapter.effects_to_commands(&mut effects, &mut mail).translated, 3);

        let mut next = LegacyInputs::new();
        adapter.event_to_inputs(HostEvent::NavPlanned(Ok(21)), &mut next).unwrap();
        assert_eq!(next.outcomes.navigator.take(), Some(NavigatorOutcome::PlanFinished { token: plan, route: 21 }));

        let preview = DetourPreview { cost_delta_m: 10, total_distance_m: 500, rejoin_m: 900, ascent_m: None };
        adapter.event_to_inputs(HostEvent::DetourPlanned(Ok(preview)), &mut next).unwrap();
        assert_eq!(next.outcomes.navigator.take(), Some(NavigatorOutcome::DetourFinished { token: detour, preview }));

        adapter.event_to_inputs(HostEvent::DetourCommitted(Err(NavError::NoPath)), &mut next).unwrap();
        assert_eq!(
            next.outcomes.navigator.take(),
            Some(NavigatorOutcome::Failed { token: commit, error: NavigatorError::Plan(NavError::NoPath) })
        );

        adapter.event_to_inputs(HostEvent::SettingsPersisted { revision: 5 }, &mut next).unwrap();
        assert_eq!(next.outcomes.settings.take(), Some(SettingsOutcome::Persisted { token: settings, revision: 5 }));

        let report = DfuScanReport::new("v1", "v2", false);
        adapter.event_to_inputs(HostEvent::DfuScanned(Ok(report.clone())), &mut next).unwrap();
        assert_eq!(next.outcomes.dfu.take(), Some(DfuOutcome::ScanFinished { token: scan, report }));

        adapter.event_to_inputs(HostEvent::CardScanned { free_bytes: Some(4096) }, &mut next).unwrap();
        assert_eq!(
            next.outcomes.storage_info.take(),
            Some(StorageInfoOutcome::Measured { token: card, free_bytes: 4096 })
        );

        // With the scan answered, the install goes out under its own class and its own token.
        let mut effects = EffectSlots::new();
        effects.dfu.try_put(DfuEffect::ArmInstall { token: install }).unwrap();
        assert_eq!(adapter.effects_to_commands(&mut effects, &mut mail).translated, 1);
        adapter.event_to_inputs(HostEvent::DfuInstallBegan, &mut next).unwrap();
        assert_eq!(next.outcomes.dfu.take(), Some(DfuOutcome::InstallBegan { token: install }));
        assert!(adapter.pending().is_empty(), "every class was answered exactly once");
    }

    /// An event nobody asked for is an explicit error, not a forged answer — and the second copy of
    /// an already-answered event is exactly that case, so a duplicate reply cannot reach a domain
    /// twice.
    #[test]
    fn an_event_with_no_pending_token_is_refused() {
        let mut ops = Ops::new();
        let mut adapter = LegacyAdapter::new();
        let mut mail = mailbox();
        let mut next = LegacyInputs::new();

        for event in [
            HostEvent::NavPlanned(Ok(1)),
            HostEvent::DetourPlanned(Err(NavError::Exhausted)),
            HostEvent::DetourCommitted(Ok(2)),
            HostEvent::SettingsPersisted { revision: 1 },
            HostEvent::DfuScanned(Err(DfuScanError::NotFound)),
            HostEvent::DfuInstallFailed(DfuInstallError::NoCard),
            HostEvent::CardScanned { free_bytes: None },
        ] {
            let class = event_reply(&event).expect("a reply-producing event");
            assert_eq!(
                adapter.event_to_inputs(event, &mut next),
                Err(LegacyError::NoPendingOperation(class)),
                "{class:?} was never requested"
            );
        }
        assert!(!next.outcomes.has_pending(), "a refused event reached no domain");

        // A real operation, answered — and then answered again.
        let token = ops.storage.issue();
        let mut effects = EffectSlots::new();
        effects.storage_info.try_put(StorageInfoEffect::MeasureFreeSpace { token }).unwrap();
        adapter.effects_to_commands(&mut effects, &mut mail);
        adapter.event_to_inputs(HostEvent::CardScanned { free_bytes: Some(8) }, &mut next).unwrap();
        assert_eq!(
            adapter.event_to_inputs(HostEvent::CardScanned { free_bytes: Some(8) }, &mut next),
            Err(LegacyError::NoPendingOperation(LegacyReply::CardScan)),
            "the correlation slot is consumed by the answer it addressed"
        );
    }

    /// A domain outcome slot that still holds an unconsumed answer is a refusal, not an overwrite:
    /// the adapter never displaces a terminal result the pass has not seen.
    #[test]
    fn a_full_outcome_slot_is_refused_rather_than_overwritten() {
        let mut ops = Ops::new();
        let mut adapter = LegacyAdapter::new();
        let mut mail = mailbox();
        let mut next = LegacyInputs::new();

        let first = ops.storage.issue();
        let mut effects = EffectSlots::new();
        effects.storage_info.try_put(StorageInfoEffect::MeasureFreeSpace { token: first }).unwrap();
        adapter.effects_to_commands(&mut effects, &mut mail);
        adapter.event_to_inputs(HostEvent::CardScanned { free_bytes: Some(1) }, &mut next).unwrap();

        let second = ops.storage.issue();
        let mut effects = EffectSlots::new();
        effects.storage_info.try_put(StorageInfoEffect::MeasureFreeSpace { token: second }).unwrap();
        adapter.effects_to_commands(&mut effects, &mut mail);
        assert_eq!(
            adapter.event_to_inputs(HostEvent::CardScanned { free_bytes: Some(2) }, &mut next),
            Err(LegacyError::OutcomeSlotFull(LegacyReply::CardScan))
        );
        assert_eq!(
            next.outcomes.storage_info.take(),
            Some(StorageInfoOutcome::Measured { token: first, free_bytes: 1 }),
            "the unconsumed answer stands"
        );

        // …and the refused answer is still addressable, because a refusal did not consume the
        // correlation slot. An executor that offers it again once the pass has caught up is
        // answered, rather than told nobody asked.
        assert!(adapter.pending().holds(LegacyReply::CardScan));
        adapter.event_to_inputs(HostEvent::CardScanned { free_bytes: Some(2) }, &mut next).unwrap();
        assert_eq!(
            next.outcomes.storage_info.take(),
            Some(StorageInfoOutcome::Measured { token: second, free_bytes: 2 }),
            "the second measurement was not lost to the busy slot"
        );
    }

    /// Two operations of one domain, in sequence, both completing — the correlation slot is per
    /// *operation*, not per boot.
    ///
    /// Worth its own test because the pass's three machine-owning domains cannot do this on the
    /// adapter path at all (see the module docs on the in-flight latch): this pins that the limit
    /// belongs to the unanswerable domains, and not to the adapter's bookkeeping.
    #[test]
    fn two_operations_of_one_domain_complete_in_sequence() {
        let mut ops = Ops::new();
        let mut adapter = LegacyAdapter::new();
        let mut mail = mailbox();
        let mut next = LegacyInputs::new();

        let mut measure = |adapter: &mut LegacyAdapter, token| {
            let mut effects = EffectSlots::new();
            effects.storage_info.try_put(StorageInfoEffect::MeasureFreeSpace { token }).unwrap();
            assert_eq!(adapter.effects_to_commands(&mut effects, &mut mail).translated, 1);
            assert_eq!(mail.pop(), Some(HostCommand::ScanCardFree));
        };

        let first = ops.storage.issue();
        measure(&mut adapter, first);
        adapter.event_to_inputs(HostEvent::CardScanned { free_bytes: Some(1) }, &mut next).unwrap();
        assert_eq!(
            next.outcomes.storage_info.take(),
            Some(StorageInfoOutcome::Measured { token: first, free_bytes: 1 })
        );
        assert!(adapter.pending().is_empty(), "the answer freed the class");

        let second = ops.storage.issue();
        measure(&mut adapter, second);
        adapter.event_to_inputs(HostEvent::CardScanned { free_bytes: Some(2) }, &mut next).unwrap();
        assert_eq!(
            next.outcomes.storage_info.take(),
            Some(StorageInfoOutcome::Measured { token: second, free_bytes: 2 }),
            "the second operation is answered under its own token"
        );
        assert_ne!(first, second, "and the two are distinguishable");
    }

    /// Uploads, warnings and the boot's update verdict become named external facts — no token, no
    /// slot, nothing claimed to have been asked for.
    #[test]
    fn uploads_warnings_and_the_boot_verdict_become_external_facts() {
        let mut adapter = LegacyAdapter::new();
        let mut next = LegacyInputs::new();

        adapter
            .event_to_inputs(HostEvent::RouteUploaded { id: 3, replaced: true, elevation: None }, &mut next)
            .unwrap();
        adapter.event_to_inputs(HostEvent::TripUploaded { id: 4, replaced: false }, &mut next).unwrap();
        adapter.event_to_inputs(HostEvent::Warning(WarningFlags::NO_GPS), &mut next).unwrap();
        adapter.event_to_inputs(HostEvent::Warning(WarningFlags::MAP_SLOW), &mut next).unwrap();
        adapter.event_to_inputs(HostEvent::UpdateConfirmed(clamp("v9")), &mut next).unwrap();

        assert!(!next.outcomes.has_pending(), "a fact is not an answer to anything");
        assert!(adapter.pending().is_empty(), "and it consumes no correlation slot");
        assert_eq!(next.facts.take_route_upload(), Some(RouteUpload { id: 3, replaced: true, elevation: None }));
        assert_eq!(next.facts.take_trip_upload(), Some(TripUpload { id: 4, replaced: false }));
        let warnings = next.facts.take_warnings();
        assert!(warnings.contains(WarningFlags::NO_GPS) && warnings.contains(WarningFlags::MAP_SLOW), "warnings OR");
        assert!(matches!(next.facts.take_update_result(), Some(UpdateResult::Confirmed(_))));

        // A second unconsumed boot verdict is the one fact merge that can fail, and it is surfaced.
        adapter.event_to_inputs(HostEvent::UpdateConfirmed(clamp("v9")), &mut next).unwrap();
        assert_eq!(
            adapter.event_to_inputs(HostEvent::UpdateConfirmed(clamp("v9")), &mut next),
            Err(LegacyError::Fact(FactMergeError::UpdateResultUnconsumed))
        );
    }

    /// The legacy protocol reports that the store moved without saying to what, so the adapter
    /// supplies the revision — one edge per commit, monotonic, under a single legacy store identity.
    #[test]
    fn each_store_change_becomes_a_fresh_store_revision() {
        let mut adapter = LegacyAdapter::new();
        let mut next = LegacyInputs::new();

        adapter.event_to_inputs(HostEvent::StoreChanged, &mut next).unwrap();
        let first = next.facts.store_revision().expect("a commit is a revision");
        adapter.event_to_inputs(HostEvent::StoreChanged, &mut next).unwrap();
        let second = next.facts.store_revision().expect("and so is the next one");

        assert_eq!(first.store, second.store, "the legacy protocol has exactly one store");
        assert_ne!(first.revision, second.revision, "two commits are two edges, never one");
    }

    // ==================== the derived levels ====================

    /// The two derived needs become their legacy cues and carry no token — the adapter takes `&self`
    /// to say so structurally, and the mailbox coalesces a repeat of the level.
    #[test]
    fn derived_needs_become_untokened_legacy_cues() {
        let adapter = LegacyAdapter::new();
        let mut mail = mailbox();

        let ride = RideTrackKey { ride: 6, source: Revision::new(2), view: Revision::ZERO };
        let route = NavPreviewKey { route: 7, source: Revision::new(3), view: Revision::ZERO };
        let needs = DerivedNeeds { ride_track: Some(ride), nav_preview: Some(route) };

        adapter.needs_to_commands(&needs, &mut mail);
        adapter.needs_to_commands(&needs, &mut mail);
        assert_eq!(
            drain(&mut mail).as_slice(),
            [HostCommand::LoadRideTrack { id: 6 }, HostCommand::RefreshNavPreview],
            "a level re-emitted in one pass coalesces"
        );
        assert!(adapter.pending().is_empty(), "a level is not an operation");

        adapter.needs_to_commands(&DerivedNeeds::NONE, &mut mail);
        assert!(mail.is_empty(), "nothing needed, nothing asked");
    }

    /// The feeder helpers' whole job: attach the key the legacy `set_*` call omitted, so an answer
    /// about the wrong subject is recognisable as such.
    #[test]
    fn the_feeder_helpers_attach_the_complete_key() {
        let adapter = LegacyAdapter::new();
        let mut next = LegacyInputs::new();

        let ride = RideTrackKey { ride: 6, source: Revision::new(2), view: Revision::ZERO };
        adapter.feed_ride_track(ride, DerivedResult::Filled, &mut next);
        assert_eq!(next.derived.ride_track, Some(DerivedInput::filled(ride)));
        assert!(next.derived.nav_preview.is_none(), "the two levels are separate slots");

        let route = NavPreviewKey { route: 7, source: Revision::new(3), view: Revision::ZERO };
        adapter.feed_nav_preview(route, DerivedResult::Failed, &mut next);
        assert_eq!(next.derived.nav_preview, Some(DerivedInput::failed(route)));
        assert!(!next.outcomes.has_pending(), "a keyed fill is not an operation result");
    }
}
