/**
 * Several GPX files at once: the small pure pieces behind the "add as a trip?" dialog.
 *
 * The uploads themselves are ordinary `sendRoute` calls the page sequences; what lives here is
 * only what a test can hold still — the ordering and the suggested trip name.
 */

import { truncateUtf8 } from "../format";
import { TRIP_NAME_MAX } from "./manage";

/**
 * Sort dropped files the way a rider numbered them: natural order ("day 2" before "day 10"),
 * case-insensitive. `DataTransfer` order is whatever the OS felt like; filenames are the one
 * ordering hint the drop actually carries.
 */
export function sortForTrip<T extends { name: string }>(files: readonly T[]): T[] {
    return [...files].sort((a, b) =>
        a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" }),
    );
}

/**
 * A trip name suggested from the files' common prefix — "tmb-day1.gpx, tmb-day2.gpx" → "tmb".
 * Empty when the names share nothing usable, so the dialog shows a blank field rather than a
 * guess nobody typed.
 */
export function commonPrefixName(filenames: readonly string[]): string {
    if (filenames.length === 0) return "";
    const stems = filenames.map(stem);
    let prefix = stems[0];
    for (const name of stems.slice(1)) {
        let i = 0;
        while (i < prefix.length && i < name.length && prefix[i].toLowerCase() === name[i].toLowerCase()) i++;
        prefix = prefix.slice(0, i);
        if (!prefix) return "";
    }
    // Trim the numbering tail ("tmb-day" → "tmb", "Etappe_" → "Etappe"): digits and separators
    // always; an ordinal word only when a separator precedes it, so a name that *is* the word
    // ("Etappe") survives. A two-character residue is noise, not a name.
    const base = prefix.replace(/[\s._-]*\d*$/, "").replace(/[\s._-]+$/, "");
    const shorn = base.replace(/[\s._-]+(day|stage|etappe|part|tag)$/i, "");
    const trimmed = shorn || base;
    return trimmed.length >= 3 ? truncateUtf8(trimmed, TRIP_NAME_MAX) : "";
}

/** A filename without its extension or any path. */
function stem(filename: string): string {
    return filename.replace(/^.*[\\/]/, "").replace(/\.[^./\\]+$/, "");
}
