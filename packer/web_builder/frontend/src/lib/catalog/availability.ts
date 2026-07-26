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
 * **Nothing sets this yet, and that is not an oversight.** C3 (#902) landed the
 * device session, and it does not carry this number: `identity.version` is the
 * *protocol* version (`PROTOCOL_VERSION`, 2 — the wire contract, not the map
 * format), and `info.firmwareRevision` is a release string like `0.4.0+abc1234`
 * that only maps to an OBCM version through a table nothing here has. Deriving
 * one would mean guessing what a firmware build can read, and guessing wrong
 * means either refusing a map that works or offering one that doesn't — both
 * worse than §6(c)'s stated fallback, which is exactly what this tier does with
 * no device: offer the download and state the version.
 *
 * So the state is designed, tested and reachable through `setDevice()`; closing
 * it needs the device to *say* which OBCM version it reads — a field in the
 * identity read, or a firmware→format table published beside the catalog.
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
