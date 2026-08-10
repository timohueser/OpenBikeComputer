import Foundation
import OBCWeatherWire

/// The one seam between the job and the fetch/build machinery, so tests can script bundles (and
/// failures) without a network. ``WeatherAssembler`` is the shipping conformer.
public protocol WeatherAssembling: Sendable {
    func assemble(request: WeatherRequest, generation: UInt32, now: Date) async throws
        -> BuiltWeatherBundle
}

extension WeatherAssembler: WeatherAssembling {}

/// The durable two-connection weather job (WX9, epic #1185 phase C).
///
/// The whole exchange, end to end:
///
/// ```
/// discovered ─▶ readingContext ─▶ fetching ─▶ bundleReady ─▶ uploading ─▶ complete
///                    │                │            │              │
///                    └── retry ◀──────┴────────────┴──────────────┘  (bounded, ladder-aware)
/// ```
///
/// **Two short connections, never a held link.** The context read leg and the upload leg are the
/// only BLE the job ever does, and everything between them — manifest, corridor tiles, MET hourly,
/// the OBCW build — runs with the radio idle. The `WeatherDeviceLink` one-shots own their own
/// deadlines; the engine owns the order.
///
/// **Durability is the checkpoint, not the process.** The job record is persisted after every
/// externally visible edge (`WeatherJobPhase`), so app suspension between the connections, a
/// CoreBluetooth state-restoration relaunch, or a plain crash resumes from the last edge: a
/// persisted context is never re-read, a persisted bundle is never re-fetched. iOS background
/// wakes are short (~10–30 s); each phase fits one wake and the checkpoint carries the job across
/// the gaps.
///
/// **Retries lean on the device.** The firmware re-raises an unanswered request on its own
/// 5/10/20-minute ladder with the *same* request id (§11.3), so the phone does not need — and must
/// not run — an aggressive local loop. A retryable failure records a short cooldown and waits for
/// the next trigger; a fresh device discovery overrides the cooldown (the device asking again *is*
/// the ladder). The attempt budget bounds the phone's total spend per request.
///
/// **Idempotence is the contract's gift.** A duplicate upload answers `committed` and finishes the
/// request (§11.6), so the engine re-uploads persisted bytes after any ambiguous outcome without a
/// dedup dance.
public actor WeatherJobEngine {
    public struct Configuration: Sendable {
        /// Total attempts (any phase) before the job is abandoned to the device's ladder.
        public var maxAttempts: Int
        /// Cooldown after a retryable failure; a fresh device discovery overrides it.
        public var retryCooldown: TimeInterval
        /// A persisted job older than this is stale — the forecast it was fetching is no longer
        /// worth finishing; drop it and let the device re-raise.
        public var jobLifetime: TimeInterval
        /// A built bundle older than this is rebuilt rather than uploaded as old weather.
        public var bundleMaxAge: TimeInterval

        public init(
            maxAttempts: Int = 6,
            retryCooldown: TimeInterval = 30,
            jobLifetime: TimeInterval = 2 * 3_600,
            bundleMaxAge: TimeInterval = 15 * 60
        ) {
            self.maxAttempts = maxAttempts
            self.retryCooldown = retryCooldown
            self.jobLifetime = jobLifetime
            self.bundleMaxAge = bundleMaxAge
        }
    }

    /// What woke the engine — decides cooldown handling and whether a read leg is owed.
    public enum Trigger: Equatable, Sendable {
        /// A Weather Request advertisement was discovered (foreground scan or background wake):
        /// the device is asking *now*, so run the read leg and ignore any local cooldown.
        case deviceRaisedRequest
        /// A context read completed autonomously in the transport (the state-restoration path
        /// finishes the read with no caller); the engine takes it from the snapshot.
        case contextRead(WeatherDeviceRequestSnapshot, readConnectedMilliseconds: Int?)
        /// App launch / foreground: finish whatever the checkpoint says is owed, honouring
        /// cooldowns. Nothing persisted means nothing to do.
        case resume
        /// The rider tapped *Retry now* on the WX13 screen. Like `.resume` it finishes only what
        /// the checkpoint already owes: with nothing persisted it is a no-op, because the phone
        /// cannot invent a request the device never raised.
        ///
        /// "Only what is owed" **includes the context read**, when that is the phase the checkpoint
        /// is parked in. The device raised the request — the advertisement is what created the
        /// checkpoint — so re-reading it is finishing the exchange, not manufacturing one. (An
        /// earlier version of this comment claimed a tap never starts a read leg; it does, and it
        /// should, or the one failure a rider is most likely to be staring at would have no retry.)
        ///
        /// Two things separate it from `.resume`. It ignores the local cooldown: an explicit tap
        /// outranks a timer the rider cannot see. And it does not spend an attempt — the attempt
        /// budget bounds the *autonomous* work this phone does per request, and a rider asking for
        /// one more go must not be what abandons the job to the device's ladder.
        case userRetry
    }

    private let link: any WeatherDeviceLink
    private let assembler: any WeatherAssembling
    private let store: any WeatherJobStore
    private let history: any WeatherJobHistoryStore
    private let configuration: Configuration
    private let now: @Sendable () -> Date

    private var running = false
    private var queuedTrigger: Trigger?
    /// Whether the run currently in flight was started by a rider's tap. Read only by
    /// ``recordFailure(_:_:error:)``, which must not spend an attempt on one.
    private var runIsUserInitiated = false
    /// Callers parked in ``awaitIdle()`` until the run loop drains. `retryNow()` uses it so a tap
    /// that only *queues* behind a run in flight still returns when the work is actually finished —
    /// a spinner that stops on "queued" is a spinner that lies (#1198 review).
    private var idleWaiters: [CheckedContinuation<Void, Never>] = []
    /// The read leg's connected time, carried job-long for the history entry.
    private var readConnectedMilliseconds: Int?
    /// The last committed job's request id and commit time — so the transport's replayed/echoed
    /// "context read completed" event for a read this engine already answered cannot restart a
    /// finished exchange. Committing finishes the request on the device (§11.3), so the same id
    /// arriving again *right after* a commit can only be an echo of the read we consumed; a
    /// genuinely new request carries a new id (the id is per-request, ladder steps keep it).
    /// The window is short so a device reboot reusing ids can never be masked for long.
    private var lastCommitted: (requestID: UInt32, committedAt: Date)?
    private static let committedEchoWindow: TimeInterval = 120

    public init(
        link: any WeatherDeviceLink,
        assembler: any WeatherAssembling,
        store: any WeatherJobStore,
        history: any WeatherJobHistoryStore,
        configuration: Configuration = Configuration(),
        now: @escaping @Sendable () -> Date = Date.init
    ) {
        self.link = link
        self.assembler = assembler
        self.store = store
        self.history = history
        self.configuration = configuration
        self.now = now
    }

    // MARK: - Triggers

    /// Run (or continue) the job for `trigger`. One run at a time; a trigger arriving mid-run is
    /// queued and replayed once, so a discovery landing during a fetch is not lost.
    public func kick(_ trigger: Trigger) async {
        if running {
            queuedTrigger = merged(queuedTrigger, with: trigger)
            return
        }
        running = true
        var next: Trigger? = trigger
        while let current = next {
            await run(current)
            next = queuedTrigger
            queuedTrigger = nil
        }
        running = false
        let waiters = idleWaiters
        idleWaiters = []
        for waiter in waiters { waiter.resume() }
    }

    /// Suspend until no run is in flight. Returns immediately when the engine is already idle.
    func awaitIdle() async {
        guard running else { return }
        await withCheckedContinuation { idleWaiters.append($0) }
    }

    /// Prefer the trigger that carries more information: a completed read beats a bare discovery,
    /// a rider's tap beats the scene-phase resume that races it, and anything beats `.resume`.
    ///
    /// `internal` rather than `private` so the table can be pinned row by row — it is a policy
    /// decision about whose intent survives a collision, and one wrong row silently swallows a
    /// rider's press.
    func merged(_ queued: Trigger?, with incoming: Trigger) -> Trigger {
        switch (queued, incoming) {
        case (nil, _): return incoming
        case (.contextRead(let snapshot, let ms), .resume),
             (.contextRead(let snapshot, let ms), .userRetry),
             (.contextRead(let snapshot, let ms), .deviceRaisedRequest):
            return .contextRead(snapshot, readConnectedMilliseconds: ms)
        case (.deviceRaisedRequest, .resume), (.deviceRaisedRequest, .userRetry):
            return .deviceRaisedRequest
        // A queued tap outranks the scene-phase `.resume` that so often lands on top of it (open
        // the app, tap Retry now: the foreground kick and the tap race). Without this row the tap
        // was swallowed — `.resume` honours the cooldown the tap exists to waive, so the rider's
        // press became a wait they could not see (#1198 review).
        case (.userRetry, .resume): return .userRetry
        default: return incoming
        }
    }

    /// The persisted job, for ``WeatherJobControlling/pendingJob()``'s coordinate-free projection.
    /// Kept `internal` so the record — which holds the rider's position — cannot leave the module.
    func pendingRecord() -> WeatherJobRecord? { store.load() }

    // MARK: - The run

    private func run(_ trigger: Trigger) async {
        runIsUserInitiated = trigger == .userRetry
        defer { runIsUserInitiated = false }
        let now = now()
        var job = store.load() ?? WeatherJobRecord(startedAt: now, updatedAt: now)

        // A checkpoint from hours ago is not worth finishing — the weather it was fetching has
        // moved on, and the device's ladder has long since re-raised or the ride ended. It aged
        // out; it did not run out of attempts (it may have spent none) and nothing superseded it.
        if now.timeIntervalSince(job.startedAt) > configuration.jobLifetime, store.load() != nil {
            finish(job: &job, outcome: .agedOut, failure: .agedOut, at: now)
            job = WeatherJobRecord(startedAt: now, updatedAt: now)
        }

        switch trigger {
        case .resume:
            // Nothing persisted → nothing owed. Cooldown honoured: `.resume` is the polite
            // trigger, and hammering the radio from every foreground is exactly the loop the
            // issue forbids.
            guard store.load() != nil else { return }
            if let notBefore = job.notBefore, now < notBefore { return }
        case .userRetry:
            // The rider asked. Same "only finish what is owed" rule as `.resume` — the phone never
            // manufactures a request — but the cooldown is waived, because a tap is a person
            // waiting rather than a timer ticking.
            guard store.load() != nil else { return }
            job.notBefore = nil
        case .deviceRaisedRequest:
            // The device is advertising *now*: any cooldown is overridden (its ladder outranks
            // ours) and a job without a snapshot starts at the read leg.
            job.notBefore = nil
        case .contextRead(let snapshot, let ms):
            if let last = lastCommitted, snapshot.requestID == last.requestID,
               now.timeIntervalSince(last.committedAt) < Self.committedEchoWindow {
                return  // an echo of the read whose job already committed — nothing owed
            }
            readConnectedMilliseconds = ms ?? readConnectedMilliseconds
            adopt(snapshot: snapshot, into: &job, at: now)
        }

        await advance(&job)
    }

    /// Fold a completed context read into the job — the correlation/discard rule.
    ///
    /// Same request id with a bundle already built → keep the bundle, go upload. A different
    /// request id (a *new* request, not a ladder step — the ladder keeps the id, §11.2) or a rider
    /// who has left the built bundle's window → the bundle no longer answers; discard it and
    /// rebuild from the fresh snapshot. The device-side rule makes the lenient path safe: even a
    /// bundle uploaded against a superseded id is accepted if it is newer (§11.6), so "rebuild"
    /// here is about answering well, not about permission.
    private func adopt(
        snapshot: WeatherDeviceRequestSnapshot, into job: inout WeatherJobRecord, at now: Date
    ) {
        job.notBefore = nil
        let sameRequest = job.snapshot.map { $0.requestID == snapshot.requestID }
        if let existing = job.snapshot, job.phase == .bundleReady || job.phase == .uploading {
            let stillCovered: Bool = {
                guard let lat = snapshot.latitudeMicrodegrees, let lon = snapshot.longitudeMicrodegrees
                else { return true }  // no fix in the new read → nothing proves the bundle wrong
                return job.bundleCovers(
                    latitudeMicrodegrees: Int64(lat), longitudeMicrodegrees: Int64(lon))
            }()
            let freshEnough = job.bundleBuiltAt.map {
                now.timeIntervalSince($0) <= configuration.bundleMaxAge
            } ?? false
            // The device may have accepted a bundle from another attempt since we built ours; a
            // held generation serially at-or-past ours makes our bytes stale on arrival.
            let generationStillAhead: Bool = {
                guard let held = snapshot.heldBundleGeneration, let ours = job.bundleGeneration
                else { return true }
                return serialIsNewer(ours, than: held)
            }()
            if existing.requestID == snapshot.requestID, stillCovered, freshEnough,
               generationStillAhead {
                job.snapshot = snapshot
                job.phase = .bundleReady
                persist(&job)
                return
            }
            // The old bundle is history — record why, then rebuild against the fresh snapshot.
            // *Why* is the point: a bundle the app slept past aged out, and calling that
            // "superseded" invents a newer request that never existed (#1227 follow-up). Only a
            // genuinely different request id, a rider who left the window, or a device that has
            // since taken an equal-or-newer generation is something superseding this work.
            let failure: WeatherJobFailure =
                (existing.requestID == snapshot.requestID && !freshEnough) ? .agedOut : .superseded
            history.append(historyEntry(
                for: job, outcome: failure == .agedOut ? .agedOut : .superseded,
                failure: failure, at: now))
        } else if sameRequest == false {
            // A *new* request id landing on a job that had not built anything yet still abandons
            // that job — the fetch it was paying for answers a question nobody is asking any more.
            // Without a row here the ring shows the work vanishing.
            history.append(historyEntry(
                for: job, outcome: .superseded, failure: .superseded, at: now))
        }
        // A ladder step re-reads the *same* request: it keeps the job's attempts **and** its
        // birthday. Resetting `startedAt` on every re-read would make `jobLifetime` decorative —
        // a device retrying every 5 minutes would keep a two-hour job alive indefinitely. Only a
        // genuinely new request id starts a new clock.
        let continuing = sameRequest == true
        job = WeatherJobRecord(
            id: job.id, phase: .fetching, snapshot: snapshot,
            attempts: continuing ? job.attempts : 0,
            deferrals: continuing ? job.deferrals : 0,
            startedAt: (job.snapshot == nil || continuing) ? job.startedAt : now,
            updatedAt: now)
        persist(&job)
    }

    /// Drive the job from its persisted phase to a terminal state or a recorded retryable failure.
    private func advance(_ job: inout WeatherJobRecord) async {
        while true {
            switch job.phase {
            case .readingContext:
                do {
                    let receipt = try await link.readRequestContext()
                    readConnectedMilliseconds = Int(receipt.connectedDuration / .milliseconds(1))
                    guard receipt.snapshot.carriesRequest else {
                        // §11.4's idle attribute: the device has nothing due (the advertisement
                        // was consumed by someone else, or the request was withdrawn). Nothing
                        // owed and nothing to report — a history row here would be a failure we
                        // invented.
                        store.clear()
                        readConnectedMilliseconds = nil
                        return
                    }
                    adopt(snapshot: receipt.snapshot, into: &job, at: now())
                } catch {
                    recordFailure(&job, .contextReadFailed, error: error)
                    return
                }
            case .fetching:
                guard let snapshot = job.snapshot else {
                    // A fetching phase without a snapshot is a corrupt checkpoint — restart clean.
                    job.phase = .readingContext
                    persist(&job)
                    continue
                }
                guard snapshot.weatherRequest.position != nil else {
                    // §11.4 says a fixless request is still a request to answer — but answering by
                    // the phone's own location needs CoreLocation and a permission prompt this
                    // build does not carry. Honest failure; the device re-raises once it has a fix.
                    finish(job: &job, outcome: .failed, failure: .noPosition, at: now())
                    return
                }
                do {
                    let built = try await assembler.assemble(
                        request: snapshot.weatherRequest,
                        generation: snapshot.nextGeneration,
                        now: now())
                    job.bundleBytes = built.bytes
                    job.bundleGeneration = built.bundle.generation
                    job.bundleWindow = [
                        Int64(built.bundle.bounds.southLatitudeMicrodegrees),
                        Int64(built.bundle.bounds.westLongitudeMicrodegrees),
                        Int64(built.bundle.bounds.northLatitudeMicrodegrees),
                        Int64(built.bundle.bounds.eastLongitudeMicrodegrees),
                    ]
                    job.bundleBuiltAt = now()
                    job.precipitationProductID = built.state.precipitation?.productID
                    job.noRainMapReason = built.state.noRainMapReason
                    job.phase = .bundleReady
                    persist(&job)
                } catch let error as WeatherBundleBuildError {
                    recordFailure(&job, .buildFailed, error: error)
                    return
                } catch {
                    recordFailure(&job, .fetchFailed, error: error)
                    return
                }
            case .bundleReady:
                // A bundle that sat in the checkpoint too long (the app slept through its own
                // upload window) is old weather — rebuild rather than upload it as current.
                if let builtAt = job.bundleBuiltAt,
                   now().timeIntervalSince(builtAt) > configuration.bundleMaxAge {
                    // The *same* event `adopt(snapshot:into:at:)` records when a ladder re-read
                    // finds the bundle expired, so it gets the same row. This is the resume path —
                    // no re-read, just the app waking up on its own — and it used to discard the
                    // bundle in silence: the ring showed a corridor fetch that vanished, and the
                    // two horizons the engine enforces (`jobLifetime`, `bundleMaxAge`) were only
                    // half visible (#1198 review).
                    history.append(historyEntry(
                        for: job, outcome: .agedOut, failure: .agedOut, at: now()))
                    job.bundleBytes = nil
                    job.bundleGeneration = nil
                    job.bundleWindow = nil
                    job.bundleBuiltAt = nil
                    job.phase = .fetching
                    persist(&job)
                    continue
                }
                job.phase = .uploading
                persist(&job)
            case .uploading:
                guard let bytes = job.bundleBytes else {
                    job.phase = .fetching
                    persist(&job)
                    continue
                }
                do {
                    let receipt = try await link.uploadBundle(bytes)
                    var entry = historyEntry(for: job, outcome: .committed, failure: nil, at: now())
                    entry.uploadConnectedMilliseconds = Int(receipt.connectedDuration / .milliseconds(1))
                    history.append(entry)
                    if let snapshot = job.snapshot {
                        lastCommitted = (snapshot.requestID, now())
                    }
                    store.clear()
                    readConnectedMilliseconds = nil
                    return
                } catch WeatherDeviceLinkError.bundleRejected {
                    // The device says these exact bytes are not a bundle (§11.5 `error`): the same
                    // bytes reproduce the failure, so the retry must be a *rebuild*.
                    job.bundleBytes = nil
                    job.bundleGeneration = nil
                    job.bundleWindow = nil
                    job.bundleBuiltAt = nil
                    job.phase = .fetching
                    recordFailure(&job, .bundleRejected, error: WeatherDeviceLinkError.bundleRejected)
                    return
                } catch WeatherDeviceLinkError.deviceBusy, WeatherDeviceLinkError.linkBusy {
                    // "Not now" — `busy` / `storageFull` / `notFound`, or the phone's transfer slot
                    // still held by a foreground transfer. None of these is a verdict on the bytes,
                    // so the bundle stays on disk and the retry re-sends it; and none of them is
                    // the *request's* fault, so it does not spend one of its attempts. Folding
                    // these into `bundleRejected` (as `storageFull`/`notFound` used to) threw away
                    // a good bundle and paid for a whole corridor re-fetch, six times over.
                    job.phase = .bundleReady
                    deferRetry(&job, .deviceUnavailable)
                    return
                } catch WeatherDeviceLinkError.transferCorrupted {
                    // The wire mangled correct bytes (§11.5 `crcMismatch`). Handled exactly like a
                    // drop — keep the bundle, re-send it — but recorded as itself: folding it into
                    // `uploadFailed` cost the ring the one distinction that separates "this link
                    // keeps dropping" from "this link corrupts what it carries" (#1227 follow-up).
                    job.phase = .bundleReady
                    recordFailure(
                        &job, .transferCorrupted, error: WeatherDeviceLinkError.transferCorrupted)
                    return
                } catch {
                    // Link-class failure: the persisted bytes stay valid, and a duplicate answers
                    // `committed` — so the retry re-uploads the same bytes safely.
                    job.phase = .bundleReady
                    recordFailure(&job, .uploadFailed, error: error)
                    return
                }
            }
        }
    }

    // MARK: - Failure bookkeeping

    /// "Come back later" — the device (or a foreground transfer) asked for the wait, so it costs a
    /// cooldown but not an attempt. Bounded anyway: past the attempt budget a deferral degrades
    /// into an ordinary attempt, so a permanently-full device cannot loop for the job's whole
    /// lifetime.
    private func deferRetry(_ job: inout WeatherJobRecord, _ failure: WeatherJobFailure) {
        job.deferrals += 1
        guard job.deferrals <= configuration.maxAttempts else {
            recordFailure(&job, failure, error: WeatherDeviceLinkError.deviceBusy)
            return
        }
        job.notBefore = now().addingTimeInterval(configuration.retryCooldown)
        persist(&job)
    }

    private func recordFailure(_ job: inout WeatherJobRecord, _ failure: WeatherJobFailure, error: Error) {
        // A rider's *Retry now* does not spend an attempt. The budget exists to bound what this
        // phone does **on its own** per request — six autonomous goes, then the device's ladder
        // owns it. A tap is not autonomous work, and letting taps burn the budget would mean the
        // rider's third press is what abandons their own job (#1198 review). Spam is bounded
        // elsewhere: the screen's in-flight gate allows one tap at a time, and a tap can only ever
        // finish work the device already asked for.
        if !runIsUserInitiated { job.attempts += 1 }
        if !runIsUserInitiated, job.attempts >= configuration.maxAttempts {
            // The attempt count in the entry already says "exhausted"; the reason field keeps the
            // *last* failure, which is the diagnostic that matters on the WX13 screen.
            finish(job: &job, outcome: .failed, failure: failure, at: now())
            return
        }
        job.notBefore = now().addingTimeInterval(configuration.retryCooldown)
        persist(&job)
    }

    /// Terminal: write the history entry, drop the checkpoint. The device's ladder owns whatever
    /// happens next.
    private func finish(
        job: inout WeatherJobRecord, outcome: WeatherJobHistoryEntry.Outcome,
        failure: WeatherJobFailure?, at now: Date
    ) {
        history.append(historyEntry(for: job, outcome: outcome, failure: failure, at: now))
        store.clear()
        readConnectedMilliseconds = nil
    }

    private func persist(_ job: inout WeatherJobRecord) {
        job.updatedAt = now()
        store.save(job)
    }

    private func historyEntry(
        for job: WeatherJobRecord, outcome: WeatherJobHistoryEntry.Outcome,
        failure: WeatherJobFailure?, at now: Date
    ) -> WeatherJobHistoryEntry {
        WeatherJobHistoryEntry(
            startedAt: job.startedAt, finishedAt: now,
            requestID: job.snapshot?.requestID ?? 0,
            outcome: outcome, failureReason: failure, phaseReached: job.phase,
            attempts: job.attempts, bundleByteCount: job.bundleBytes?.count,
            readConnectedMilliseconds: readConnectedMilliseconds,
            precipitationProductID: job.precipitationProductID,
            noRainMapReason: job.noRainMapReason)
    }
}

/// RFC-1982-style serial "newer than" over `u32` generations — the same comparison the device's
/// §11.6 disposition uses, mirrored here so the phone predicts the device's verdict instead of
/// guessing it.
func serialIsNewer(_ candidate: UInt32, than reference: UInt32) -> Bool {
    guard candidate != reference else { return false }
    let distance = candidate &- reference
    return distance < 0x8000_0000
}
