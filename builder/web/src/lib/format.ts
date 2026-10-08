/** Human byte sizes. Host-neutral on purpose: the build card shows a result
 *  size on every tier, and importing it from the dev host's job tracker would
 *  drag SSE polling into bundles that have no server to poll. */
export function formatBytes(n: number): string {
    if (n >= 1 << 20) return (n / (1 << 20)).toFixed(1) + " MB";
    if (n >= 1 << 10) return (n / (1 << 10)).toFixed(1) + " KB";
    return n + " B";
}

/** Throughput, for a transfer whose speed is worth stating — a map upload is minutes long, and the
 *  number is what makes that credible instead of alarming. */
export function formatRate(bytesPerSecond: number): string {
    return formatBytes(Math.round(bytesPerSecond)) + "/s";
}

/** A rough remaining time. Rounded coarsely and prefixed by the caller with
 *  "about", because a to-the-second estimate off a moving average is a
 *  precision nobody has. */
export function formatDuration(seconds: number): string {
    if (seconds < 60) return `${Math.max(1, Math.round(seconds))} s`;
    const minutes = Math.round(seconds / 60);
    if (minutes < 60) return `${minutes} min`;
    const hours = Math.floor(minutes / 60);
    const rest = minutes % 60;
    return rest ? `${hours} h ${rest} min` : `${hours} h`;
}

/** Trim UTF-8 to a byte-sized wire field without splitting a codepoint. */
export function truncateUtf8(text: string, maxBytes: number): string {
    const encoder = new TextEncoder();
    if (encoder.encode(text).length <= maxBytes) return text;
    let out = "";
    let used = 0;
    for (const ch of text) {
        const size = encoder.encode(ch).length;
        if (used + size > maxBytes) break;
        out += ch;
        used += size;
    }
    return out;
}
