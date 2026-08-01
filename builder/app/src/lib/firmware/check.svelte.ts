/**
 * One update check per page, shared by the card that acts on it and the prompt that mentions it.
 *
 * ## Why the fetch left the card
 *
 * Two surfaces want the same answer: `components/device/FirmwareCard.svelte`, which is still the
 * only place that downloads, stages or asks the device to install anything, and
 * `components/UpdatePrompt.svelte` (#1002), which only says that there is something worth looking
 * at and points at the card. Two `onMount` fetches would cost the update host two requests for one
 * question and could disagree with each other for the lifetime of a page. So the check lives here
 * and both read it.
 *
 * ## The privacy rule survived the move
 *
 * The check runs on {@link FirmwareCheck.ensure}, and **nothing calls it until a device is
 * connected**. With nothing to compare against, a request buys the visitor nothing and costs them a
 * connection to a host they did not ask to talk to. That was C4's rule when the fetch was the
 * card's `onMount`; moving it here does not weaken it — it gives it one place to hold, and both
 * callers are `$effect`s gated on a live session.
 *
 * ## What the rider has already answered
 *
 * A proactive prompt needs a memory or it is nagging. One ledger, keyed by
 * `(device serial, offered version)` and kept in `localStorage` the way the working config and the
 * thumbnail cache are, records the pairs the rider has answered — dismissed it, followed it to the
 * card, or went ahead and staged that very version. A newer release, or a different device, is a
 * new question and is asked. The ledger suppresses the **prompt** and nothing else: the card's own
 * flow — the offer, the send button, the file picker — never reads it.
 */

import { compareVersions, fetchFirmwareRelease, updateStatus, type FirmwareRelease } from "./release";

/**
 * The slice of `localStorage` the ledger needs — injectable, so the store is tested against a Map
 * rather than a browser global. The same seam, for the same reason, as `device/thumbs.svelte.ts`.
 */
export interface LedgerStorage {
    get(key: string): string | null;
    /** May throw (quota, private mode). The ledger treats that as "this browser will not remember",
     *  which costs the rider a repeated prompt and nothing else. */
    set(key: string, value: string): void;
}

const LEDGER_KEY = "obcm.fwPromptAnswered";

/** Answers kept, most recent last. A handful of devices times a handful of releases is already
 *  generous; the cap only exists so an ancient key cannot grow without bound. */
export const LEDGER_CAP = 32;

/** One answer's key. Serial-scoped, so answering for one device says nothing about another. */
function answerKey(serial: string, version: string): string {
    return `${serial}@${version}`;
}

export class FirmwareCheck {
    /** The published release. Null both before the check has run and when nothing is published —
     *  {@link checked} is what tells those apart. */
    release = $state<FirmwareRelease | null>(null);
    /** The check could not be made sense of: unreachable, or a manifest that parsed as nothing. A
     *  404 is not this — that is the ordinary "nothing published yet" answer. */
    failed = $state(false);
    /** True once a check has settled, either way. */
    checked = $state(false);

    private readonly storage: LedgerStorage | null;
    private answered = $state<string[]>([]);
    private started: Promise<void> | null = null;

    constructor(storage: LedgerStorage | null = browserStorage()) {
        this.storage = storage;
        this.answered = this.load();
    }

    /**
     * Make the check, at most once per page — later callers await the first one's promise.
     *
     * **Only ever called with a device connected** (see the module comment). The options exist for
     * the tests and for nothing else; the app calls this with no arguments.
     */
    ensure(options: { url?: string; fetch?: typeof globalThis.fetch } = {}): Promise<void> {
        this.started ??= this.run(options);
        return this.started;
    }

    private async run(options: { url?: string; fetch?: typeof globalThis.fetch }): Promise<void> {
        try {
            this.release = await fetchFirmwareRelease(options);
        } catch {
            this.failed = true;
        } finally {
            this.checked = true;
        }
    }

    /**
     * The release worth interrupting this device's rider about, or null — which is the answer for
     * every state except one.
     *
     * `available` and unanswered is the whole condition: `current`, `ahead` and `no-release` have
     * nothing to say proactively, and `unknown` is #773's locked refusal — a device reporting a git
     * hash is never offered an update, least of all in a popup.
     */
    offer(serial: string | null | undefined, running: string | null | undefined): FirmwareRelease | null {
        const release = this.release;
        // No serial is no scope for an answer: prompting without being able to remember the
        // dismissal would prompt again on the next render.
        if (!release || !serial) return null;
        if (updateStatus(running, release.version) !== "available") return null;
        return this.isAnswered(serial, release.version) ? null : release;
    }

    /** Whether this device's rider has answered this version or a newer one. A release-channel
     * rollback is not a new question, and alternate spellings of one semantic version are not
     * separate questions. */
    isAnswered(serial: string, version: string): boolean {
        const prefix = `${serial}@`;
        return this.answered.some((entry) => {
            if (!entry.startsWith(prefix)) return false;
            const answeredVersion = entry.slice(prefix.length);
            const order = compareVersions(answeredVersion, version);
            return order === null ? answeredVersion === version : order >= 0;
        });
    }

    /**
     * Record that the question has been answered for `(serial, version)` — dismissed, followed to
     * the card, or acted on by staging that version. Any of the three means the prompt has nothing
     * left to add, so all three land here.
     */
    answer(serial: string | null | undefined, version: string): void {
        if (!serial) return;
        // A late completion from an older prompt must not regress the ledger or consume another
        // slot. The browser is single-threaded, but foreground flows can still settle out of order.
        if (this.isAnswered(serial, version)) return;
        const key = answerKey(serial, version);
        const next = [...this.answered.filter((entry) => entry !== key), key].slice(-LEDGER_CAP);
        this.answered = next;
        try {
            this.storage?.set(LEDGER_KEY, JSON.stringify(next));
        } catch {
            // Quota or a denied store: the answer holds for this page and is forgotten on reload,
            // which is the harmless direction to fail in.
        }
    }

    private load(): string[] {
        try {
            const raw = this.storage?.get(LEDGER_KEY);
            if (!raw) return [];
            const parsed: unknown = JSON.parse(raw);
            if (!Array.isArray(parsed)) return [];
            return parsed.filter((entry): entry is string => typeof entry === "string").slice(-LEDGER_CAP);
        } catch {
            // A corrupt ledger asks one extra time. Rewriting it is not worth a branch.
            return [];
        }
    }
}

/** `localStorage` behind the seam — or null where the platform denies it (then memory-only). */
function browserStorage(): LedgerStorage | null {
    try {
        const ls = globalThis.localStorage;
        // A throwing store (denied cookies) is caught here; a missing one (node) falls through.
        ls.getItem(LEDGER_KEY);
        return {
            get: (key) => ls.getItem(key),
            set: (key, value) => ls.setItem(key, value),
        };
    } catch {
        return null;
    }
}

/** The one check the app makes. */
export const firmwareCheck = new FirmwareCheck();
