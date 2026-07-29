/**
 * "Is there a newer firmware than the one running?" — the check, and the version dialect it needs.
 *
 * #773 locks the distribution end of this and the builder does not relitigate it: the request is an
 * anonymous GET with no accounts and nothing sent about the device, for the one manifest at
 * {@link FIRMWARE_MANIFEST_URL}. This page reads that manifest and compares it against the running
 * version; it never decides what an update *is*.
 *
 * ## Provisional, and honest about it
 *
 * U3 (#773) is the sub-issue that makes `release.yml` emit that manifest and mirror it to the
 * bucket, and it has not landed — so today the fetch 404s and the UI says there is nothing
 * published yet, rather than pretending. {@link parseFirmwareManifest} implements the shape U3's
 * description names (version, size, sha256, a notes URL) and rejects anything else loudly. When U3
 * publishes the real file, the only thing that can need changing is this parser.
 *
 * ## The version dialect, which is the part with teeth
 *
 * A firmware revision string comes from the Device Information Service. Today that is
 * `CARGO_PKG_VERSION+git-hash`; after #773's U1 it prefers the *installed* OBCU container's
 * version. Either way, some devices report something that is not a release version at all — a
 * probe-flashed dev build reports a bare hash — and #773 states the consequence plainly: the app
 * **cannot parse it as a version and never offers an auto-update**. That is a locked behaviour, not
 * a limitation to work around, so {@link compareVersions} refuses rather than guesses, and
 * {@link updateStatus} answers `unknown`.
 */

/** What the manifest says about the newest published build. */
export interface FirmwareRelease {
    /** The release version, as tagged (`1.4.0`, `v1.4.0`). */
    readonly version: string;
    /** Bytes of the `UPDATE.BIN` container. */
    readonly bytes: number;
    /** Lowercase hex SHA-256 of the container. */
    readonly sha256: string;
    /** Where the container is fetched from. */
    readonly url: string;
    /** Release notes, if the manifest points at any. */
    readonly notes?: string;
}

/**
 * Where the manifest is served from, and deliberately **not** GitHub.
 *
 * A release asset's stable download URL 302s to blob storage that sends no
 * `access-control-allow-origin` header, so a browser `fetch` for it fails — which kills the check
 * in both hosts that have a browser in them, the static web builder and the desktop app's
 * WKWebView. (The JSON API does send CORS headers, but it is the rate-limited surface #773's body
 * set out to avoid.) So #773's 2026-07-29 planning comment locks the serving end: `release.yml`
 * mirrors `manifest.json` + `UPDATE.BIN` to R2 behind this domain, under a per-channel `fw/`
 * prefix, and **GitHub Releases stays the source of truth** — the publish trigger, the immutable
 * archive, the release notes, the ELFs. Nothing about the trust chain moves with the bytes: the
 * sha256 here (and the Ed25519 signature U3 adds) is what says an image is genuine, not the host
 * that served it.
 *
 * Overridable so a test never touches the network.
 */
export const FIRMWARE_MANIFEST_URL = "https://updates.openbikecomputer.com/fw/manifest.json";

export class FirmwareManifestError extends Error {
    constructor(message: string) {
        super(message);
        this.name = "FirmwareManifestError";
    }
}

/**
 * Parse a whole manifest body.
 *
 * Whole, not streamed, and every required field checked before any of it is used — the same posture
 * `OBCC_Spec.md` §7 takes for the map catalog, for the same reason: a half-understood manifest that
 * offers a download is worse than no manifest.
 */
export function parseFirmwareManifest(body: string): FirmwareRelease {
    let doc: unknown;
    try {
        doc = JSON.parse(body);
    } catch (cause) {
        throw new FirmwareManifestError(`the firmware manifest is not a JSON document: ${describe(cause)}`);
    }
    if (typeof doc !== "object" || doc === null || Array.isArray(doc)) {
        throw new FirmwareManifestError("the firmware manifest is not a JSON object.");
    }
    const raw = doc as Record<string, unknown>;
    const version = str(raw, "version");
    if (!parseVersion(version)) {
        throw new FirmwareManifestError(`the firmware manifest's version "${version}" is not a release version.`);
    }
    const bytes = raw.bytes ?? raw.size;
    if (typeof bytes !== "number" || !Number.isInteger(bytes) || bytes <= 0) {
        throw new FirmwareManifestError("the firmware manifest's size is missing or not a positive integer.");
    }
    const sha256 = str(raw, "sha256").toLowerCase();
    if (!/^[0-9a-f]{64}$/.test(sha256)) {
        throw new FirmwareManifestError("the firmware manifest's sha256 is not a 64-character hex digest.");
    }
    const url = str(raw, "url");
    if (!/^https:\/\//.test(url)) {
        throw new FirmwareManifestError("the firmware manifest's url must be an https URL.");
    }
    const notes = typeof raw.notes === "string" && raw.notes ? raw.notes : undefined;
    return { version, bytes, sha256, url, ...(notes ? { notes } : {}) };
}

function str(raw: Record<string, unknown>, key: string): string {
    const value = raw[key];
    if (typeof value !== "string" || !value) {
        throw new FirmwareManifestError(`the firmware manifest's ${key} is missing or not a string.`);
    }
    return value;
}

/**
 * Fetch the published manifest.
 *
 * `null` means *there is nothing published* — a 404, which is the ordinary answer until #773's U3
 * ships — and is not an error. A malformed manifest **is** an error, because that one means
 * something is wrong at the publishing end and hiding it would hide it forever.
 */
export async function fetchFirmwareRelease(
    options: { url?: string; signal?: AbortSignal; fetch?: typeof globalThis.fetch } = {},
): Promise<FirmwareRelease | null> {
    const get = options.fetch ?? globalThis.fetch;
    const response = await get(options.url ?? FIRMWARE_MANIFEST_URL, {
        signal: options.signal,
        // No credentials, no headers worth naming: #773's "anonymous GET, no accounts" rule.
        credentials: "omit",
    });
    if (response.status === 404) return null;
    if (!response.ok) {
        throw new FirmwareManifestError(`the firmware manifest could not be fetched (HTTP ${response.status}).`);
    }
    return parseFirmwareManifest(await response.text());
}

// --- the version dialect ------------------------------------------------------

interface Version {
    major: number;
    minor: number;
    patch: number;
    /** A pre-release tag (`rc1`), which sorts *before* the same triple without one. */
    pre: string | null;
}

/**
 * Parse a release version, or `null` for anything that is not one.
 *
 * Accepts an optional leading `v`, a three-part numeric core, an optional `-pre` tag and optional
 * `+build` metadata — the last of which is *ignored*, so `1.2.0+abc1234` (what DIS reports today)
 * and `1.2.0` are the same version. A bare git hash parses as nothing at all, which is the point.
 */
export function parseVersion(text: string): Version | null {
    const match = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/.exec(text.trim());
    if (!match) return null;
    return { major: +match[1], minor: +match[2], patch: +match[3], pre: match[4] ?? null };
}

/**
 * Order two version strings: negative if `a` is older, 0 if equal, positive if newer.
 *
 * `null` when either side is not a release version. Callers must treat that as "cannot say" rather
 * than as "not newer" — #773's rule is that an unparseable running version means no update is ever
 * offered, and collapsing `null` into `0` would silently offer one.
 */
export function compareVersions(a: string, b: string): number | null {
    const left = parseVersion(a);
    const right = parseVersion(b);
    if (!left || !right) return null;
    if (left.major !== right.major) return left.major - right.major;
    if (left.minor !== right.minor) return left.minor - right.minor;
    if (left.patch !== right.patch) return left.patch - right.patch;
    if (left.pre === right.pre) return 0;
    // A pre-release precedes its release; between two pre-releases, plain string order is enough
    // for the one thing this decides ("is the published one newer than mine").
    if (left.pre === null) return 1;
    if (right.pre === null) return -1;
    return left.pre < right.pre ? -1 : 1;
}

/**
 * What to say about a device running `running` when the newest published build is `latest`.
 *
 * - `unknown` — the running version is not a release version (a probe-flashed dev build). No
 *   update is offered; #773 locks that.
 * - `ahead` — the device is running something newer than what is published. Says so; does not
 *   offer to downgrade.
 *
 * An unparseable running version answers `unknown` **even when nothing is published**, and that
 * ordering is deliberate: what makes a dev build undecidable is the version it reports, not the
 * absence of a manifest. Answering `no-release` there would hide the state #773's U4/U5 amendment
 * asks the apps to show — "development build, automatic updates paused" — behind whichever
 * publication happens to exist that day.
 */
export function updateStatus(
    running: string | null | undefined,
    latest: string | null,
): "no-release" | "unknown" | "current" | "available" | "ahead" {
    if (running && !parseVersion(running)) return "unknown";
    if (!latest) return "no-release";
    if (!running) return "unknown";
    const order = compareVersions(running, latest);
    if (order === null) return "unknown";
    if (order < 0) return "available";
    return order > 0 ? "ahead" : "current";
}

function describe(cause: unknown): string {
    return cause instanceof Error ? cause.message : String(cause);
}
