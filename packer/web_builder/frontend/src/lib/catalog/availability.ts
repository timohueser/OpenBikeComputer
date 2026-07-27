// The three states the picker paints, and the one device fact they turn on.
//
// OBCC §6(c) is the consumer's half of the version law: with a known target
// firmware, an artifact whose `obcm_version` that firmware doesn't read MUST
// NOT be offered, and SHOULD be shown as unsupported *with the reason* rather
// than hidden — hiding it makes a rider's out-of-date firmware look like a gap
// in coverage. With no device attached the download MAY be offered, stating the
// version, which is the hosted tier's normal case: this path is designed to
// work with nothing plugged in and on browsers that have no USB at all.

import type { CatalogArtifact } from "./manifest";
import type { RegionEntry } from "./regions";

/**
 * What the catalog needs to know about a connected device: the OBCM format
 * version its firmware reads. One fact, because that is the whole of §6(c).
 *
 * The reader supports exactly one OBCM version at a time (OBCM_Spec.md: v10 is
 * the only supported version; earlier maps get repacked), so this is a single
 * number rather than a range, and `supports()` is the one place that assumption
 * lives if a future device ever reads two.
 *
 * **The device says it; this file never guesses it.** E1 (#911) added the byte:
 * the identity read (spec §1) now carries `obcm_version`, sourced on the
 * firmware side straight from `obc_formats::obcm::VERSION` — the same constant
 * the reader validates every `.obcm` header against, so what the device claims
 * and what it reads cannot drift. Before that there was nothing to read it
 * from: `identity.version` is the *protocol* version (the wire contract), and
 * `info.firmwareRevision` is a release string that maps to a format version
 * only through a table nothing here has.
 *
 * A device whose read carries no such byte (an older firmware; the store-less
 * 2-byte read) stays `null` here rather than becoming a number, and `null` is
 * the honest answer: §6(c)'s no-known-target-firmware branch offers the
 * download stating the version, which beats both refusing a map that works and
 * offering one that doesn't.
 */
export interface DeviceMapSupport {
    /** The OBCM format version this device's firmware reads. */
    obcmVersion: number;
}

/**
 * The identity read's `obcm_version` (spec §1) as a catalog device, or `null`
 * when the device did not state one.
 *
 * One line, but it is *the* line the §6(c) wiring turns on, and it lives here —
 * a plain `number | null` in, no session, no import from `lib/usb/` — so it can
 * be tested and so the device step stays a two-call component. `null` in means
 * `null` out: an older firmware (the 6-byte read) and a card-less device (the
 * 2-byte read) both leave the picker in its no-known-target-firmware branch,
 * which offers the download stating the version. The tempting shortcut — treat
 * a connected device as "surely current" — is the guess this whole field exists
 * to remove.
 */
export function deviceFromIdentity(obcmVersion: number | null | undefined): DeviceMapSupport | null {
    return typeof obcmVersion === "number" ? { obcmVersion } : null;
}

export type ArtifactState =
    | { kind: "available" }
    | { kind: "unsupported"; artifactObcm: number; deviceObcm: number };

/** A region, across all its presets. `not-baked` includes "no artifact at all". */
export type RegionState = "available" | "unsupported" | "not-baked";

export function supports(device: DeviceMapSupport, artifact: CatalogArtifact): boolean {
    return device.obcmVersion === artifact.obcm_version;
}

export function artifactState(
    artifact: CatalogArtifact,
    device: DeviceMapSupport | null,
): ArtifactState {
    if (device && !supports(device, artifact)) {
        return {
            kind: "unsupported",
            artifactObcm: artifact.obcm_version,
            deviceObcm: device.obcmVersion,
        };
    }
    return { kind: "available" };
}

/**
 * A region's state on the map. A region with artifacts none of which the
 * connected device can read reads as `unsupported`, not as `not-baked`: the
 * maps exist, and telling a rider "not covered" when the truth is "your
 * firmware is behind" sends them to the wrong fix.
 */
export function regionState(entry: RegionEntry, device: DeviceMapSupport | null): RegionState {
    if (!entry.artifacts.length) return "not-baked";
    const anyReadable = entry.artifacts.some((a) => artifactState(a, device).kind === "available");
    return anyReadable ? "available" : "unsupported";
}

/**
 * True when this artifact was baked with an older revision of its preset (§3).
 * Worth surfacing as "older styling" and nothing more: the file is valid,
 * readable and complete, and a consumer MUST NOT refuse it.
 */
export function stylingLagsPreset(artifact: CatalogArtifact, preset: { version: number }): boolean {
    return artifact.preset_version < preset.version;
}
