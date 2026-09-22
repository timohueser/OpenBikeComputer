use crate::catalog_state::CatalogState;
use crate::dfu::{DfuFailure, DfuInstallError, DfuScanError, DfuScanReport};
use crate::screen::{self, MapTransfer, Screen, Stack, WarningFlags};

/// One committed route upload, as the pass's fact stage posts it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct UploadEvent {
    /// The committed route's durable object id, resolved to a catalog index at delivery time.
    pub(crate) id: crate::CatalogObjectId,
    /// The upload replaced the actively-navigated route, snapshotted at arrival. Adoption already
    /// happened, so the card is the info-only "ROUTE UPDATED" one and not a choice prompt.
    pub(crate) active_replace: bool,
    /// The route's mini elevation sparkline, built by the host from the just-committed OBCR, or
    /// `None` when the route carries no elevation. Only the idle "ROUTE RECEIVED" card draws it.
    pub(crate) elevation: Option<[u8; obc_route::SPARKLINE_BUCKETS]>,
}

/// What the single pending-upload slot holds: one committed route or trip upload. One slot for both
/// kinds keeps most-recent-wins across the whole popup family, and because a trip object always
/// arrives after its member routes, a burst of route events capped by the trip event collapses to
/// the one "TRIP RECEIVED" prompt.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PendingUpload {
    Route(UploadEvent),
    /// The committed trip's durable object id, validated against the trip catalog at delivery time.
    Trip {
        id: crate::CatalogObjectId,
    },
}

/// The one-time post-update verdict this boot produced. The board's boot-outcome reconcile yields
/// at most one of the two, so they share one slot.
#[derive(Debug, Clone)]
pub(crate) enum BootUpdate {
    /// The trial image confirmed: the running version, for the "Updated to vX" toast.
    Confirmed(heapless::String<32>),
    /// The armed update is not what is running: the typed verdict plus the staged version the arm
    /// marker recorded, if it survived.
    Failed(DfuFailure, Option<heapless::String<32>>),
}

/// A terminal answer to the DFU wait screen the rider or a remote `installFw` opened. It lands only
/// into the wait it belongs to; an answer whose wait is gone is dropped, never pushed loose.
#[derive(Debug, Clone)]
pub(crate) enum DfuLanding {
    /// The card scan answered: the confirm screen, or the error card.
    Scanned(Result<DfuScanReport, DfuScanError>),
    /// The install began: the static pre-reset card replaces the spinner.
    InstallBegan,
    /// The install refused or failed before the reset.
    InstallFailed(DfuInstallError),
}

/// The six host-pushed card families, in delivery order: every `High` row before every `Low` one.
/// The discriminant indexes [`POLICY`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Passkey = 0,
    MapTransfer = 1,
    DfuLanding = 2,
    Upload = 3,
    Warning = 4,
    UpdateToast = 5,
}

/// Delivery rank. `High` families land first within a sweep, and a `Low` family never covers the
/// passkey card, the one `High` card that competes for the same glass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Priority {
    High,
    Low,
}

/// One policy row. See [`POLICY`] for the whole table.
struct Policy {
    family: Family,
    priority: Priority,
    /// Whether a charging hold postpones this family, which keeps a host-pushed screen from
    /// appearing or vanishing under a finger mid-charge.
    ///
    /// False for the DFU landing and only for it. Every screen a DFU answer replaces is a modal
    /// wait that binds no gesture, so there is no hold target to protect, and the install-began
    /// answer has no next pass to be retried on: the board posts it, renders that one frame, and
    /// hands the panel to a warm reset that never paints again. Deferring it would latch the
    /// animated "Preparing update…" spinner onto the MIP for the whole install.
    defer_on_hold: bool,
}

/// The card policy, one row per family, read instead of six reconcilers.
///
/// `priority` and `defer_on_hold` are what the sweep reads. The conflict rule and the revalidation
/// are what each family's arm below does: they need the stack, the catalogs or the flag set, so
/// they are code rather than data, and each arm names its row. The timeout is deliberately not a
/// column: the 30 s deadline lives on the popup screens, which is also what arms the timed wake
/// that gets a parked device back here, and a second copy would only ever disagree.
const POLICY: [Policy; 6] = [
    Policy { family: Family::Passkey, priority: Priority::High, defer_on_hold: true },
    Policy { family: Family::MapTransfer, priority: Priority::High, defer_on_hold: true },
    Policy { family: Family::DfuLanding, priority: Priority::High, defer_on_hold: false },
    Policy { family: Family::Upload, priority: Priority::Low, defer_on_hold: true },
    Policy { family: Family::Warning, priority: Priority::Low, defer_on_hold: true },
    Policy { family: Family::UpdateToast, priority: Priority::Low, defer_on_hold: true },
];

/// The whole pending state is resident on the board, so it stays register-sized per family. The DFU
/// scan report's two version strings dominate it.
const _: () = assert!(core::mem::size_of::<CardScheduler>() <= 256, "CardScheduler grew — re-check the slots");

/// The table is indexed by `Family as usize` and iterated as the delivery order, so the rows must
/// stay in discriminant order.
const _: () = {
    let mut i = 0;
    while i < POLICY.len() {
        assert!(POLICY[i].family as usize == i, "POLICY rows must stay in Family discriminant order");
        i += 1;
    }
};

/// Every stack screen a sweep needs to find: the card families themselves plus the three DFU wait
/// screens a landing replaces. Anything else on the stack is a rider-opened screen the scheduler
/// does not touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardKind {
    Passkey,
    MapTransfer,
    /// Any upload-family screen: the three received popups and the manual Route-swap prompt, which
    /// an incoming prompt replaces by the same rule.
    Upload,
    Warning,
    DfuCheck,
    DfuProgress,
    DfuInstalling,
}

/// Which tracked kind a screen is, if any. Every lookup below shares this one classification, so
/// what counts as an upload popup is written once.
fn kind_of(s: &Screen) -> Option<CardKind> {
    Some(match s {
        Screen::Passkey(_) => CardKind::Passkey,
        Screen::MapTransfer(_) => CardKind::MapTransfer,
        Screen::RouteReceived(_) | Screen::RouteUpdated(_) | Screen::TripReceived(_) | Screen::RouteSwap(_) => {
            CardKind::Upload
        }
        Screen::Warning(_) => CardKind::Warning,
        Screen::DfuCheck(_) => CardKind::DfuCheck,
        Screen::DfuProgress(_) => CardKind::DfuProgress,
        Screen::DfuInstalling(_) => CardKind::DfuInstalling,
        _ => return None,
    })
}

/// Where a tracked kind sits on the stack, lowest slot first. Every arm reads the stack through
/// this rather than through a cached index set, so no arm can act on a stale picture of a stack an
/// earlier arm just moved. The stack is at most [`MAX_DEPTH`](crate::screen::MAX_DEPTH) slots, so a
/// lookup is a handful of discriminant compares.
fn find(stack: &Stack, kind: CardKind) -> Option<usize> {
    stack.iter().position(|s| kind_of(s) == Some(kind))
}

/// The single landing door for every scheduler card: rewrite `at` when the family's conflict rule
/// targets an open slot, otherwise push. It holds the one overflow assert, loud in debug and a
/// silent no-op in release. Removals are each family's own `stack.remove`.
///
/// Returns whether the card is on the stack. A rewrite always is; a push can fail on a full stack,
/// and every one-shot fact is consumed only on a `true`. Otherwise a release build would mark a
/// warning shown, or take a boot verdict, for a card the rider never saw, and neither would ever
/// come back.
#[must_use]
fn land(stack: &mut Stack, at: Option<usize>, screen: Screen) -> bool {
    // A card does not land on a device that is switching off. The sweep that would land it runs in
    // the same `handle_input` as the guarded hold the rider completed, and either arm below would
    // take `power_off_requested` back to `false` and cancel the shutdown. Returning `false` is
    // already the "not shown" answer, so the one-shot fact behind the card is preserved.
    if crate::screen::powering_off(stack) {
        return false;
    }
    // A card never lands under a sheet: an open drawer comes off first, in both arms. Overlays sit
    // above every card slot, so `at` is unaffected by the pop.
    crate::screen::close_drawers(stack);
    match at {
        Some(i) => {
            stack[i] = screen;
            true
        }
        None => {
            let r = stack.push(screen);
            debug_assert!(r.is_ok(), "screen stack overflow — raise MAX_DEPTH");
            r.is_ok()
        }
    }
}

/// Whether the upload-family screen at a slot is a host-pushed popup rather than the rider's own
/// menu-opened Route-swap prompt. The passkey card's conflict rule keys on this.
fn is_received_popup(s: &Screen) -> bool {
    match s {
        Screen::RouteReceived(_) | Screen::RouteUpdated(_) | Screen::TripReceived(_) => true,
        Screen::RouteSwap(s) => s.is_received(),
        _ => false,
    }
}

/// Whether the upload-family screen at a slot is past its auto-close deadline. The manual swap
/// prompt never expires; it waits for the rider.
fn upload_expired(s: &Screen, now_ms: u32) -> bool {
    match s {
        Screen::RouteReceived(s) => s.expired(now_ms),
        Screen::RouteUpdated(s) => s.expired(now_ms),
        Screen::TripReceived(s) => s.expired(now_ms),
        Screen::RouteSwap(s) => s.expired(now_ms),
        _ => false,
    }
}

/// The cross-component facts a sweep needs. Everything else it decides from its own slots and the
/// stack; the component never reaches back into `App`.
pub(crate) struct CardCtx<'a> {
    /// The map plane's clock: the open anchor a landing popup stamps, and the expiry `now`.
    pub(crate) now_ms: u32,
    /// A hold is charging on either plane. Suspends the stack-wide steps and every family whose
    /// row sets [`defer_on_hold`](Policy::defer_on_hold).
    pub(crate) hold_charging: bool,
    /// Resolves a pending upload's durable id at delivery time.
    pub(crate) catalogs: &'a CatalogState,
    /// Whether a ride is recording — which upload card a route commit becomes.
    pub(crate) tracking: bool,
}

/// The named pending slots plus the one sweep. One slot per family and no untyped queue, so what is
/// waiting is answered by reading six fields.
pub(crate) struct CardScheduler {
    /// The desired passkey level, re-fed every pass: `Some` wants the card up, `None` wants it gone.
    passkey: Option<u32>,
    /// The desired map-transfer level, re-fed every pass while a write runs.
    map_transfer: Option<MapTransfer>,
    /// Whether the level above is represented on the stack by a card this scheduler landed. It is
    /// what tells a dismissal from a first delivery: with the level unchanged and the card gone,
    /// the rider popped it, and re-landing it would undo the press. Cleared whenever the level
    /// changes, so a new state always re-raises.
    map_transfer_delivered: bool,
    /// The one pending upload prompt; the most recent route or trip commit wins. Carried by durable
    /// object id, never a catalog index, so a rescan between arrival and a deferred delivery cannot
    /// retarget it.
    upload: Option<PendingUpload>,
    /// Warning flags discovered but not yet shown.
    warnings: WarningFlags,
    /// Warnings already shown on a card this boot, so each flag surfaces once and a dismissed
    /// notice does not nag, while a genuinely new flag still re-opens the card. Never cleared.
    warned: WarningFlags,
    /// The one boot-result slot. A second unconsumed result is rejected at post.
    update: Option<BootUpdate>,
    /// The one terminal answer for the DFU wait currently on the stack.
    dfu: Option<DfuLanding>,
}

impl CardScheduler {
    pub(crate) const fn new() -> Self {
        CardScheduler {
            passkey: None,
            map_transfer: None,
            map_transfer_delivered: false,
            upload: None,
            warnings: WarningFlags::NONE,
            warned: WarningFlags::NONE,
            update: None,
            dfu: None,
        }
    }

    pub(crate) fn set_passkey(&mut self, passkey: Option<u32>) {
        self.passkey = passkey;
    }

    /// The live passkey as last fed.
    pub(crate) fn passkey_level(&self) -> Option<u32> {
        self.passkey
    }

    pub(crate) fn set_map_transfer(&mut self, state: Option<MapTransfer>) {
        // A changed level is a new fact and always re-raises the card, even if the rider dismissed
        // the previous one. An unchanged re-feed, the steady state fed every pass, must not.
        if state != self.map_transfer {
            self.map_transfer_delivered = false;
        }
        self.map_transfer = state;
    }

    /// Post a committed upload into the single prompt slot; the most recent wins.
    pub(crate) fn post_upload(&mut self, upload: PendingUpload) {
        self.upload = Some(upload);
    }

    /// Accumulate freshly-raised warning flags. An empty raise is a no-op.
    pub(crate) fn post_warning(&mut self, flags: WarningFlags) {
        self.warnings |= flags;
    }

    /// Post this boot's one-time update verdict. A second result arriving before the first is shown
    /// is rejected: the board's boot-outcome reconcile yields at most one.
    pub(crate) fn post_update(&mut self, result: BootUpdate) {
        if self.update.is_none() {
            self.update = Some(result);
        }
    }

    /// Post a terminal DFU answer for the wait currently on the stack; the latest wins.
    pub(crate) fn post_dfu(&mut self, landing: DfuLanding) {
        self.dfu = Some(landing);
    }

    /// The per-pass sweep, the scheduler's only stack mutation. Returns whether anything visible
    /// changed, so the caller sets the map dirty exactly once.
    ///
    /// A charging hold suspends the two stack-wide steps and every family whose row sets
    /// [`defer_on_hold`](Policy::defer_on_hold); the slots are re-fed or stay pending, so a
    /// deferral is only "try again next pass". Each arm reads the stack through [`find`], so it
    /// always sees what the arm before it did.
    pub(crate) fn sweep(&mut self, stack: &mut Stack, ctx: &CardCtx) -> bool {
        let mut changed = false;
        if !ctx.hold_charging {
            changed |= self.remove_vanished(stack);
        }
        for row in POLICY.iter() {
            changed |= self.deliver(row, stack, ctx);
        }
        if !ctx.hold_charging {
            changed |= expire_upload(stack, ctx.now_ms);
        }
        changed
    }

    /// The two level families: a level that went `None` takes its card off the stack wherever it
    /// ended up.
    fn remove_vanished(&mut self, stack: &mut Stack) -> bool {
        let mut changed = false;
        for (level_present, kind) in
            [(self.passkey.is_some(), CardKind::Passkey), (self.map_transfer.is_some(), CardKind::MapTransfer)]
        {
            if level_present {
                continue;
            }
            if let Some(i) = find(stack, kind) {
                let _ = stack.remove(i);
                changed = true;
            }
        }
        changed
    }

    /// One family's delivery, or terminal replacement, per its policy row.
    fn deliver(&mut self, row: &Policy, stack: &mut Stack, ctx: &CardCtx) -> bool {
        if row.defer_on_hold && ctx.hold_charging {
            return false;
        }
        // The rank gate: a `Low` family never covers the passkey card. What it does instead is its
        // own row's business. The upload prompt is dropped, because the object is in the menu
        // either way; the warning and the toast stay pending for a later pass.
        let outranked = row.priority == Priority::Low && find(stack, CardKind::Passkey).is_some();
        match row.family {
            Family::Passkey => self.deliver_passkey(stack),
            Family::MapTransfer => self.deliver_map_transfer(stack),
            Family::DfuLanding => self.deliver_dfu(stack),
            Family::Upload => self.deliver_upload(stack, ctx, outranked),
            Family::Warning => self.deliver_warning(stack, outranked),
            Family::UpdateToast => self.deliver_update(stack, outranked),
        }
    }

    /// Passkey. Conflict: an open received popup is replaced rather than stacked over, because it
    /// is advisory, while the rider's own menu-opened swap prompt stays put under the card. A
    /// changed code rewrites the open card, so no rider types a dead pairing code into their phone;
    /// the same code re-fed each pass is no change, so the steady state never re-dirties.
    fn deliver_passkey(&mut self, stack: &mut Stack) -> bool {
        let Some(passkey) = self.passkey else { return false };
        if let Some(i) = find(stack, CardKind::Passkey) {
            let Screen::Passkey(card) = &mut stack[i] else { return false };
            if card.passkey() == passkey {
                return false;
            }
            *card = screen::PasskeyScreen::new(passkey);
            return true;
        }
        if let Some(i) = find(stack, CardKind::Upload) {
            if is_received_popup(&stack[i]) {
                let _ = stack.remove(i);
            }
        }
        land(stack, None, Screen::Passkey(screen::PasskeyScreen::new(passkey)))
    }

    /// Map transfer. Conflict: the open card is rewritten in place, never stacked. An unchanged
    /// re-feed reports no change, so a multi-minute write does not repaint the panel continuously.
    fn deliver_map_transfer(&mut self, stack: &mut Stack) -> bool {
        let Some(state) = self.map_transfer else {
            self.map_transfer_delivered = false;
            return false;
        };
        match find(stack, CardKind::MapTransfer) {
            Some(i) => {
                let Screen::MapTransfer(card) = &mut stack[i] else { return false };
                self.map_transfer_delivered = true;
                if card.state() == state {
                    return false;
                }
                card.set_state(state);
                true
            }
            // The dismissal. A terminal card pops itself on a press, and the press and this sweep
            // are stages of the same pass, so re-landing here would put it back before the pass
            // ends: the platform's "the card was up and no longer is" latch would never observe it,
            // the level would never clear, and the rider would be locked on the card until reboot.
            None if self.map_transfer_delivered => false,
            None => {
                self.map_transfer_delivered =
                    land(stack, None, Screen::MapTransfer(screen::MapTransferScreen::new(state)));
                self.map_transfer_delivered
            }
        }
    }

    /// DFU landing. Conflict: the answer replaces the wait it belongs to. Revalidation: that wait
    /// must still be on the stack, or the answer is dropped, because the rider pressed Back. The
    /// install-began card is the one exception: with no spinner up, the debug arm, it pushes. This
    /// row never defers on a hold; see [`Policy::defer_on_hold`].
    fn deliver_dfu(&mut self, stack: &mut Stack) -> bool {
        let Some(landing) = self.dfu.take() else { return false };
        // Only the install-began answer can reach a push; the other two replace a wait, which
        // cannot fail. So that is the one variant a full stack can bounce back into the slot.
        let pushes = matches!(landing, DfuLanding::InstallBegan);
        let (at, screen) = match landing {
            DfuLanding::Scanned(result) => {
                let Some(i) = find(stack, CardKind::DfuCheck) else { return false };
                let card = match result {
                    Ok(report) => Screen::DfuConfirm(screen::DfuConfirmScreen::new(report)),
                    Err(e) => Screen::DfuError(screen::DfuErrorScreen::new(e)),
                };
                (Some(i), card)
            }
            DfuLanding::InstallBegan => {
                (find(stack, CardKind::DfuProgress), Screen::DfuInstalling(screen::DfuInstallingScreen::new()))
            }
            // Whichever install wait sits lower: the spinner, or the terminal card that already
            // replaced it.
            DfuLanding::InstallFailed(reason) => {
                let wait = stack
                    .iter()
                    .position(|s| matches!(kind_of(s), Some(CardKind::DfuProgress | CardKind::DfuInstalling)));
                let Some(i) = wait else { return false };
                (Some(i), Screen::DfuError(screen::DfuErrorScreen::new_install(reason)))
            }
        };
        if land(stack, at, screen) {
            return true;
        }
        debug_assert!(pushes, "a DFU answer that replaces its wait cannot fail to land");
        if pushes {
            self.dfu = Some(DfuLanding::InstallBegan);
        }
        false
    }

    /// Upload prompt. Conflict: the incoming prompt replaces the upload family in place, so
    /// consecutive uploads never stack and selection resets with the fresh screen. Revalidation:
    /// the durable id must still resolve in the rescanned catalog, or the prompt is dropped.
    fn deliver_upload(&mut self, stack: &mut Stack, ctx: &CardCtx, outranked: bool) -> bool {
        let Some(ev) = self.upload else { return false };
        self.upload = None; // delivered or dropped, never queued behind the passkey card
        if outranked {
            return false;
        }
        let card = match ev {
            PendingUpload::Route(ev) => {
                let Some(i) = ctx.catalogs.route_index_of(ev.id) else { return false };
                if ev.active_replace {
                    Screen::RouteUpdated(screen::RouteUpdatedScreen::new(i, ctx.now_ms))
                } else if ctx.tracking {
                    Screen::RouteSwap(screen::RouteSwapScreen::received(i, ctx.now_ms))
                } else {
                    Screen::RouteReceived(screen::RouteReceivedScreen::new(i, ctx.now_ms, ev.elevation))
                }
            }
            // The trip card is the same whether idle or tracking, because a trip is a folder and
            // there is nothing to swap onto. The screen keeps the durable id, so it needs no remap.
            PendingUpload::Trip { id } => {
                if !ctx.catalogs.trips().iter().any(|t| t.id == id) {
                    return false;
                }
                Screen::TripReceived(screen::TripReceivedScreen::new(id, ctx.now_ms))
            }
        };
        let at = find(stack, CardKind::Upload);
        land(stack, at, card)
    }

    /// Warning. Conflict: fresh flags merge into the open card rather than stacking a second one.
    /// Revalidation: only the not-yet-shown subset is surfaced, so an already-acknowledged flag
    /// re-raised each pass stays quiet.
    fn deliver_warning(&mut self, stack: &mut Stack, outranked: bool) -> bool {
        let fresh = self.warnings & !self.warned;
        if fresh.is_empty() {
            self.warnings = WarningFlags::NONE; // nothing new: drop any stale re-raise
            return false;
        }
        if outranked {
            return false; // still pending; retried once the card clears
        }
        match find(stack, CardKind::Warning) {
            Some(i) => {
                if let Screen::Warning(s) = &mut stack[i] {
                    s.add(fresh);
                }
            }
            // A full stack leaves the flags pending rather than marking them shown for a card that
            // never opened — they would otherwise never surface again this boot.
            None if !land(stack, None, Screen::Warning(screen::WarningScreen::new(fresh))) => return false,
            None => {}
        }
        self.warned |= fresh;
        self.warnings = WarningFlags::NONE;
        true
    }

    /// Update toast. Conflict: pushed once, over whatever is up. Revalidation: the slot itself is
    /// the fact, and taking it is what makes the card show once per boot.
    fn deliver_update(&mut self, stack: &mut Stack, outranked: bool) -> bool {
        if outranked {
            return false;
        }
        let Some(result) = self.update.as_ref() else { return false };
        let card = match result {
            BootUpdate::Confirmed(version) => Screen::DfuUpdated(screen::DfuUpdatedScreen::new(version)),
            BootUpdate::Failed(why, staged) => Screen::DfuFailed(screen::DfuFailedScreen::new(*why, staged.as_deref())),
        };
        if !land(stack, None, card) {
            return false; // no room: the verdict keeps its slot and shows on a later pass
        }
        self.update = None;
        true
    }
}

/// A timeout is a dismissal: an upload card past its deadline is removed exactly as Back would
/// remove it, and nothing else changes. The deadline is the screen's, which is also what armed the
/// timed wake that got a parked device to this line.
fn expire_upload(stack: &mut Stack, now_ms: u32) -> bool {
    let Some(i) = find(stack, CardKind::Upload) else { return false };
    if !upload_expired(&stack[i], now_ms) {
        return false;
    }
    let _ = stack.remove(i);
    true
}

impl CardScheduler {
    pub(crate) fn reconcile_journey(
        &mut self,
        stack: &mut Stack,
        hold: bool,
        arrival: bool,
        resume: bool,
        accepted: bool,
    ) -> bool {
        let mut changed = false;
        if !hold {
            if let Some(i) = stack
                .iter()
                .position(|s| matches!(s, Screen::Journey(s) if (!s.resume && !arrival) || (s.resume && accepted)))
            {
                let resumed = matches!(&stack[i], Screen::Journey(s) if s.resume && accepted);
                if resumed {
                    screen::apply(stack, screen::Transition::Root(Screen::Map(screen::MapScreen::new())));
                } else {
                    stack.remove(i);
                }
                changed = true;
            }
            if (arrival || resume)
                && !stack.iter().any(|s| matches!(s, Screen::Journey(_)))
                && !stack.iter().any(|s| {
                    matches!(
                        s,
                        Screen::RideRecovery(_)
                            | Screen::DfuCheck(_)
                            | Screen::DfuProgress(_)
                            | Screen::DfuInstalling(_)
                    )
                })
                && find(stack, CardKind::Passkey).is_none()
            {
                changed |= land(stack, None, Screen::Journey(screen::JourneyScreen::new(resume)));
            }
        }
        changed
    }
}

#[cfg(test)]
impl CardScheduler {
    /// Whether every slot is unset and no warning has been raised or shown: the
    /// [`new`](CardScheduler::new) state. The destructure is exhaustive, so a new slot must state
    /// its empty value here too.
    pub(crate) fn is_empty(&self) -> bool {
        let CardScheduler { passkey, map_transfer, map_transfer_delivered, upload, warnings, warned, update, dfu } =
            self;
        passkey.is_none()
            && map_transfer.is_none()
            // Implied by the line above, and asserted anyway: the latch being cleared whenever the
            // level goes `None` is the whole reason a dismissal is legible.
            && !*map_transfer_delivered
            && upload.is_none()
            && *warnings == WarningFlags::NONE
            && *warned == WarningFlags::NONE
            && update.is_none()
            && dfu.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::{BootUpdate, DfuLanding};
    use crate::screen::{MapTransfer, WarningFlags, MAX_DEPTH};
    use crate::{App, AppState, BleLink, BleStatus, Gesture, Screen};
    use obc_ports::InputClock;

    /// A passkey card, raised through the real BLE seam rather than a hand-pushed screen.
    fn pair(app: &mut App, passkey: Option<u32>) {
        app.set_ble_status(BleStatus { link: BleLink::Connected, passkey, paired: false });
    }

    /// The warning-fact contract: a raised flag opens the card, further flags coalesce onto the
    /// open one, any press dismisses it, and each flag is shown once. An already-shown flag stays
    /// quiet, but a genuinely new one re-opens the card with only itself.
    #[test]
    fn warning_card_opens_coalesces_and_shows_each_flag_once() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        assert!(matches!(app.top_screen(), Screen::Home(_)));

        app.on_warning(WarningFlags::NONE);
        assert!(matches!(app.top_screen(), Screen::Home(_)), "an empty warning is a no-op");

        app.on_warning(WarningFlags::NO_GPS);
        match app.top_screen() {
            Screen::Warning(w) => assert!(w.flags().contains(WarningFlags::NO_GPS)),
            _ => panic!("a raised warning opens the card"),
        }

        app.on_warning(WarningFlags::STORAGE_ERROR);
        assert_eq!(app.ui.stack.len(), 2, "the new flag joins the open card, not a second one");
        match app.top_screen() {
            Screen::Warning(w) => {
                assert!(w.flags().contains(WarningFlags::NO_GPS));
                assert!(w.flags().contains(WarningFlags::STORAGE_ERROR));
            }
            _ => panic!("still the one card"),
        }

        app.apply_gesture(Gesture::Back);
        assert!(matches!(app.top_screen(), Screen::Home(_)), "dismiss pops the card");

        app.on_warning(WarningFlags::NO_GPS);
        assert!(matches!(app.top_screen(), Screen::Home(_)), "an already-shown flag stays quiet");

        app.on_warning(WarningFlags::NO_COMPASS);
        match app.top_screen() {
            Screen::Warning(w) => {
                assert!(w.flags().contains(WarningFlags::NO_COMPASS));
                assert!(!w.flags().contains(WarningFlags::NO_GPS), "the re-opened card carries only the new flag");
            }
            _ => panic!("a new flag re-opens the card"),
        }
    }

    /// A scan answer lands in the "Checking update..." wait the System menu pushed, swapping it for
    /// the confirm screen or the error card. With no wait on the stack it is a no-op, because the
    /// rider pressed Back.
    #[test]
    fn dfu_scan_result_replaces_the_check_wait() {
        use crate::dfu::{DfuScanError, DfuScanReport};
        let mk = |v: &str| {
            let mut s = heapless::String::new();
            let _ = s.push_str(v);
            s
        };
        let report =
            DfuScanReport { installed: mk("v1.0.0-0-gaaa"), staged: mk("v1.1.0-3-gbbb"), first_install: false };

        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.post_dfu_landing(DfuLanding::Scanned(Ok(report.clone())));
        assert!(!app.ui.stack.iter().any(|s| matches!(s, Screen::DfuConfirm(_))), "no wait ⇒ answer dropped");

        let _ = app.ui.stack.push(Screen::DfuCheck(crate::screen::DfuCheckScreen::new()));
        app.post_dfu_landing(DfuLanding::Scanned(Ok(report)));
        assert!(matches!(app.top_screen(), Screen::DfuConfirm(_)), "Ok swaps the wait for the confirm");

        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let _ = app.ui.stack.push(Screen::DfuCheck(crate::screen::DfuCheckScreen::new()));
        app.post_dfu_landing(DfuLanding::Scanned(Err(DfuScanError::TooFragmented)));
        match app.top_screen() {
            Screen::DfuError(e) => {
                assert_eq!(e.reason(), crate::screen::DfuErrorReason::Scan(DfuScanError::TooFragmented))
            }
            _ => panic!("Err swaps the wait for the error card"),
        }
    }

    /// An install failure lands in the "Preparing update..." spinner the confirm swapped in,
    /// replacing it with the error card. With no progress screen on the stack it is a no-op,
    /// symmetric with the scan answer. The error-to-card mapping is pinned, including the re-scan
    /// bucket folding to a scan reason so it shares the scan copy.
    #[test]
    fn dfu_install_failure_replaces_the_progress_spinner() {
        use crate::dfu::DfuInstallError;
        use crate::screen::DfuErrorReason;

        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.post_dfu_landing(DfuLanding::InstallFailed(DfuInstallError::NoCard));
        assert!(!app.ui.stack.iter().any(|s| matches!(s, Screen::DfuError(_))), "no spinner ⇒ answer dropped");

        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let _ = app.ui.stack.push(Screen::DfuProgress(crate::screen::DfuProgressScreen::new()));
        app.post_dfu_landing(DfuLanding::InstallFailed(DfuInstallError::Recording));
        match app.top_screen() {
            Screen::DfuError(e) => assert_eq!(e.reason(), DfuErrorReason::Install(DfuInstallError::Recording)),
            _ => panic!("a refusal swaps the spinner for the error card"),
        }
        assert!(!app.ui.stack.iter().any(|s| matches!(s, Screen::DfuProgress(_))), "the spinner is gone");

        // An arm-time re-scan failure folds to a plain scan reason.
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let _ = app.ui.stack.push(Screen::DfuProgress(crate::screen::DfuProgressScreen::new()));
        app.post_dfu_landing(DfuLanding::InstallFailed(DfuInstallError::Scan(crate::dfu::DfuScanError::Damaged)));
        match app.top_screen() {
            Screen::DfuError(e) => assert_eq!(e.reason(), DfuErrorReason::Scan(crate::dfu::DfuScanError::Damaged)),
            _ => panic!("the re-scan bucket lands the error card"),
        }

        // A failure past the terminal-frame swap lands the error card on the installing card.
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let _ = app.ui.stack.push(Screen::DfuInstalling(crate::screen::DfuInstallingScreen::new()));
        app.post_dfu_landing(DfuLanding::InstallFailed(DfuInstallError::SnapshotFailed));
        match app.top_screen() {
            Screen::DfuError(e) => assert_eq!(e.reason(), DfuErrorReason::Install(DfuInstallError::SnapshotFailed)),
            _ => panic!("a post-swap failure swaps the installing card for the error card"),
        }
        assert!(!app.ui.stack.iter().any(|s| matches!(s, Screen::DfuInstalling(_))), "the installing card is gone");
    }

    /// The install-began answer swaps the "Preparing update..." spinner for the static installing
    /// card, the pre-reset frame the panel holds through the install. With no spinner up, the debug
    /// command's direct arm, it pushes the card instead.
    #[test]
    fn show_dfu_installing_swaps_the_spinner_or_pushes() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let _ = app.ui.stack.push(Screen::DfuProgress(crate::screen::DfuProgressScreen::new()));
        app.post_dfu_landing(DfuLanding::InstallBegan);
        assert!(matches!(app.top_screen(), Screen::DfuInstalling(_)), "the spinner became the installing card");
        assert!(!app.ui.stack.iter().any(|s| matches!(s, Screen::DfuProgress(_))), "the spinner is gone");

        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.post_dfu_landing(DfuLanding::InstallBegan);
        assert!(matches!(app.top_screen(), Screen::DfuInstalling(_)), "pushed with no spinner up");
    }

    /// The map-transfer card's whole life cycle, the seam a multi-minute SD write is visible
    /// through: one card for the whole transfer, progress rewritten in place, an unchanged re-feed
    /// repainting nothing, a terminal state dismissable by a press while a receiving one is not,
    /// and `None` closing it.
    #[test]
    fn map_transfer_card_opens_updates_and_closes() {
        use crate::screen::{MapTransfer, MapTransferError};
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let cards = |app: &App| app.ui.stack.iter().filter(|s| matches!(s, Screen::MapTransfer(_))).count();

        assert!(!app.map_transfer_card_up(), "no card before a transfer");
        app.set_map_transfer(Some(MapTransfer::Receiving { received_kib: 0, total_kib: 400_000 }));
        assert!(app.map_transfer_card_up(), "the first announced byte raises the card");
        assert_eq!(cards(&app), 1);

        // Progress rewrites the one card; an identical re-feed must not dirty the map, or the
        // transfer would repaint the panel continuously.
        app.set_map_transfer(Some(MapTransfer::Receiving { received_kib: 100_000, total_kib: 400_000 }));
        assert_eq!(cards(&app), 1, "progress never stacks a second card");
        app.ui.map_dirty = false;
        app.set_map_transfer(Some(MapTransfer::Receiving { received_kib: 100_000, total_kib: 400_000 }));
        assert!(!app.ui.map_dirty, "an unchanged state repaints nothing");

        // Modal while receiving: a press cannot dismiss the one explanation for the busy glass.
        app.apply_gesture(Gesture::Press);
        assert!(app.map_transfer_card_up(), "a receiving card swallows input");

        app.set_map_transfer(Some(MapTransfer::Installed));
        assert_eq!(cards(&app), 1, "the outcome replaces the progress state in place");
        app.apply_gesture(Gesture::Press);
        assert!(!app.map_transfer_card_up(), "a terminal card dismisses on a press");

        // A failure raises the card the same way, and `None` closes it silently.
        app.set_map_transfer(Some(MapTransfer::Failed(MapTransferError::Damaged)));
        assert!(app.map_transfer_card_up());
        app.set_map_transfer(None);
        assert!(!app.map_transfer_card_up(), "clearing the state removes the card");
        assert_eq!(cards(&app), 0);
    }

    /// A dismissed card must stay dismissed across sweeps, and a new level must still raise it.
    ///
    /// The sibling test above asserts the dismissal only up to the `apply_gesture` that pops it.
    /// The card is a level family, so what decides whether the rider can leave the screen is what
    /// the next sweep does, and the pass runs a sweep after every gesture batch.
    #[test]
    fn a_dismissed_card_stays_dismissed_until_the_level_changes() {
        use crate::screen::{MapTransfer, MapTransferError};
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));

        app.set_map_transfer(Some(MapTransfer::Installed));
        assert!(app.map_transfer_card_up(), "a terminal state raises the card");
        app.apply_gesture(Gesture::Press);
        assert!(!app.map_transfer_card_up(), "a terminal card dismisses on a press");

        // The steady state: the platform keeps re-feeding the same level until it observes the
        // card gone.
        app.set_map_transfer(Some(MapTransfer::Installed));
        assert!(!app.map_transfer_card_up(), "an unchanged re-feed must not resurrect a dismissed card");

        // A genuinely new fact is not a re-feed, and must be shown.
        app.set_map_transfer(Some(MapTransfer::Receiving { received_kib: 0, total_kib: 400_000 }));
        assert!(app.map_transfer_card_up(), "a new transfer raises the card again");
        app.set_map_transfer(Some(MapTransfer::Failed(MapTransferError::Damaged)));
        assert!(app.map_transfer_card_up(), "a terminal outcome replaces the progress state in place");
        app.apply_gesture(Gesture::Press);
        assert!(!app.map_transfer_card_up(), "and it dismisses too");

        // Clearing the level is still the platform's way to close it silently.
        app.set_map_transfer(None);
        assert!(!app.map_transfer_card_up());
    }

    /// A confirmed-update fact surfaces the "Updated to vX" card once on the next
    /// `advance_animations` pass. A normal boot, with no fact, pushes nothing.
    #[test]
    fn confirmed_update_pushes_the_toast_once() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.advance_animations(InputClock(1000));
        assert!(!app.ui.stack.iter().any(|s| matches!(s, Screen::DfuUpdated(_))), "a normal boot shows no toast");

        app.post_boot_update(BootUpdate::Confirmed(crate::dfu::clamp("v2.0.0-0-gccc")));
        app.advance_animations(InputClock(2000));
        assert!(matches!(app.top_screen(), Screen::DfuUpdated(_)), "the confirmed update surfaces the toast");
        app.ui.stack.pop(); // dismiss
        app.advance_animations(InputClock(3000));
        assert!(!app.ui.stack.iter().any(|s| matches!(s, Screen::DfuUpdated(_))), "shown once — the fact was consumed");
    }

    /// The failure twin: a failed-update fact surfaces the "UPDATE FAILED" card once, with the
    /// typed verdict the seam carries.
    #[test]
    fn failed_update_pushes_the_card_once() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.advance_animations(InputClock(1000));
        assert!(!app.ui.stack.iter().any(|s| matches!(s, Screen::DfuFailed(_))), "a normal boot shows no failure card");

        app.post_boot_update(BootUpdate::Failed(
            crate::dfu::DfuFailure::Reverted,
            Some(crate::dfu::clamp("v2.0.0-0-gccc")),
        ));
        app.advance_animations(InputClock(2000));
        match app.top_screen() {
            Screen::DfuFailed(card) => assert_eq!(card.why(), crate::dfu::DfuFailure::Reverted),
            _ => panic!("expected the failure card on top"),
        }
        app.ui.stack.pop(); // dismiss
        app.advance_animations(InputClock(3000));
        assert!(!app.ui.stack.iter().any(|s| matches!(s, Screen::DfuFailed(_))), "shown once — the fact was consumed");
    }

    /// No scheduler stack mutation while a hold charges. Each fact stays in its slot and lands on
    /// the first pass after the hold settles.
    #[test]
    fn a_charging_hold_delays_every_deferring_family() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.set_hold_progress(0.5, 0.0);

        pair(&mut app, Some(4242));
        app.set_map_transfer(Some(MapTransfer::Receiving { received_kib: 1, total_kib: 10 }));
        app.on_warning(WarningFlags::NO_GPS);
        app.post_boot_update(BootUpdate::Confirmed(crate::dfu::clamp("v2.0.0-0-gccc")));
        assert!(matches!(app.top_screen(), Screen::Home(_)), "nothing lands mid-hold");
        assert_eq!(app.debug_stack_len(), 1);

        // The hold settles: one sweep lands every deferred High fact.
        app.set_hold_progress(0.0, 0.0);
        app.advance_animations(InputClock(100));
        let up = |app: &App, f: fn(&Screen) -> bool| app.ui.stack.iter().any(f);
        assert!(up(&app, |s| matches!(s, Screen::Passkey(_))), "the passkey card landed");
        assert!(up(&app, |s| matches!(s, Screen::MapTransfer(_))), "the transfer card landed");
        // The two Low facts are still pending: they never cover the pairing code.
        assert!(!up(&app, |s| matches!(s, Screen::Warning(_) | Screen::DfuUpdated(_))), "Low waits behind High");
    }

    /// The DFU landing is the one row that does not defer, and it must not: it replaces modal waits
    /// that bind no gesture, and the install-began answer has no next pass to be retried on.
    #[test]
    fn a_dfu_landing_delivers_under_a_charging_hold() {
        use crate::dfu::{DfuInstallError, DfuScanReport};
        let mk = |v: &str| {
            let mut s = heapless::String::new();
            let _ = s.push_str(v);
            s
        };

        // A button is physically down when the install begins.
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let _ = app.ui.stack.push(Screen::DfuProgress(crate::screen::DfuProgressScreen::new()));
        app.set_hold_progress(0.5, 0.0);
        app.post_dfu_landing(DfuLanding::InstallBegan);
        assert!(matches!(app.top_screen(), Screen::DfuInstalling(_)), "the terminal frame lands under the hold");
        assert!(!app.ui.stack.iter().any(|s| matches!(s, Screen::DfuProgress(_))), "the spinner is gone");

        // The scan answer and the failure landing share the row, so they share the rule.
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let _ = app.ui.stack.push(Screen::DfuCheck(crate::screen::DfuCheckScreen::new()));
        app.set_hold_progress(0.9, 0.0);
        let report = DfuScanReport { installed: mk("v1"), staged: mk("v2"), first_install: false };
        app.post_dfu_landing(DfuLanding::Scanned(Ok(report)));
        assert!(matches!(app.top_screen(), Screen::DfuConfirm(_)), "the scan answer lands under the hold");

        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let _ = app.ui.stack.push(Screen::DfuProgress(crate::screen::DfuProgressScreen::new()));
        app.set_hold_progress(0.3, 0.0);
        app.post_dfu_landing(DfuLanding::InstallFailed(DfuInstallError::NoCard));
        assert!(matches!(app.top_screen(), Screen::DfuError(_)), "the failure lands under the hold");
    }

    /// The boot-verdict slot's posting rule: a second unconsumed result is rejected, and the first
    /// posted wins. That is what stops a late verdict from overwriting, or stacking a second card
    /// on, one the rider has not seen yet.
    #[test]
    fn a_second_unconsumed_boot_verdict_is_rejected() {
        let cards = |app: &App| {
            app.ui.stack.iter().filter(|s| matches!(s, Screen::DfuUpdated(_) | Screen::DfuFailed(_))).count()
        };
        let fail = || BootUpdate::Failed(crate::dfu::DfuFailure::Reverted, Some(crate::dfu::clamp("v2.0.0-0-gccc")));

        // A hold keeps the first verdict unconsumed while the other arrives.
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.set_hold_progress(0.5, 0.0);
        app.post_boot_update(BootUpdate::Confirmed(crate::dfu::clamp("v1.0.0-0-gaaa")));
        app.post_boot_update(fail());
        app.set_hold_progress(0.0, 0.0);
        app.advance_animations(InputClock(100));
        assert_eq!(cards(&app), 1, "one card, never two");
        assert!(matches!(app.top_screen(), Screen::DfuUpdated(_)), "the first verdict owns the slot");

        // Symmetric the other way round: a confirm arriving behind a failure is rejected too.
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.set_hold_progress(0.5, 0.0);
        app.post_boot_update(fail());
        app.post_boot_update(BootUpdate::Confirmed(crate::dfu::clamp("v1.0.0-0-gaaa")));
        app.set_hold_progress(0.0, 0.0);
        app.advance_animations(InputClock(100));
        assert_eq!(cards(&app), 1);
        match app.top_screen() {
            Screen::DfuFailed(card) => assert_eq!(card.why(), crate::dfu::DfuFailure::Reverted),
            _ => panic!("the first verdict owns the slot"),
        }

        // Consumed, the slot is free again: a later boot verdict is not locked out for good.
        app.ui.stack.pop();
        app.post_boot_update(BootUpdate::Confirmed(crate::dfu::clamp("v3.0.0-0-gddd")));
        assert!(matches!(app.top_screen(), Screen::DfuUpdated(_)), "the taken slot accepts the next fact");
    }

    /// The warning and the toast wait behind the pairing code, because they are still owed, while
    /// an upload prompt is dropped, because the route is in the menu either way. Both land in one
    /// sweep once the card clears.
    #[test]
    fn low_priority_facts_land_once_the_passkey_card_clears() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        pair(&mut app, Some(123_456));
        assert!(matches!(app.top_screen(), Screen::Passkey(_)));

        app.on_warning(WarningFlags::NO_GPS);
        app.post_boot_update(BootUpdate::Confirmed(crate::dfu::clamp("v2.0.0-0-gccc")));
        assert!(matches!(app.top_screen(), Screen::Passkey(_)), "the card is never covered");
        assert_eq!(app.debug_stack_len(), 2);

        pair(&mut app, None);
        assert!(matches!(app.top_screen(), Screen::DfuUpdated(_)), "the toast is the last High→Low landing");
        assert!(app.ui.stack.iter().any(|s| matches!(s, Screen::Warning(_))), "the warning landed too");
    }

    /// A full stack, both halves of the contract: the overflow stays loud in debug through the one
    /// `land` assert, and, because a one-shot fact is consumed only once its card is on the stack,
    /// the flags survive to open the card when there is room again.
    #[test]
    #[cfg(debug_assertions)]
    fn a_full_stack_fails_loudly_and_keeps_the_fact_pending() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        while app.debug_stack_len() < MAX_DEPTH {
            let _ = app.ui.stack.push(Screen::Menu(crate::screen::MenuScreen::new()));
        }

        let overflow = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            app.on_warning(WarningFlags::NO_GPS);
        }));
        assert!(overflow.is_err(), "a full stack fails loudly in debug builds");
        assert_eq!(app.debug_stack_len(), MAX_DEPTH, "and nothing landed");

        // Room appears: the flags were never consumed, so the card opens carrying them.
        app.ui.stack.pop();
        app.advance_animations(InputClock(100));
        match app.top_screen() {
            Screen::Warning(w) => assert!(w.flags().contains(WarningFlags::NO_GPS), "the fact outlived the overflow"),
            _ => panic!("the warning lands once there is room"),
        }
    }

    /// The passkey card is a level family like the transfer card, so a code that changes while the
    /// card is up rewrites it in place, and the same code re-fed every pass repaints nothing.
    #[test]
    fn a_changed_passkey_rewrites_the_open_card() {
        let code = |app: &App| match app.top_screen() {
            Screen::Passkey(card) => card.passkey(),
            _ => panic!("the passkey card is up"),
        };
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        pair(&mut app, Some(111_111));
        assert_eq!(code(&app), 111_111);
        let _ = app.take_dirty();

        pair(&mut app, Some(111_111));
        assert!(!app.ui.map_dirty, "the same code, re-fed every pass, repaints nothing");

        pair(&mut app, Some(222_222));
        assert_eq!(code(&app), 222_222, "the card shows the live code, never a stale one");
        assert_eq!(app.debug_stack_len(), 2, "rewritten in place, never a second card");
        assert!(app.ui.map_dirty, "…and the change repaints");
    }
}
