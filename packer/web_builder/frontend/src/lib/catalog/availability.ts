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
 * What the catalog needs to know about a connected device. Deliberately one
 * fact: C3 (#902) owns the USB protocol client and the device session, and this
 * is the only thing the picker asks of it. Nothing here reaches for a device —
 * `catalogStore.device` stays null until something sets it.
 *
 * The reader supports exactly one OBCM version at a time (OBCM_Spec.md: v10 is
 * the only supported version; earlier maps get repacked), so this is a single
 * number rather than a range, and `supports()` is the one place that assumption
 * lives if a future device ever reads two.
 */
export interface DeviceMapSupport {
    /** The OBCM format version this device's firmware reads. */
    obcmVersion: number;
    /** How to name the device in copy, e.g. "OpenBikeComputer". Optional. */
    label?: string;
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
