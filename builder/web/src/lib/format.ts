/** Human byte sizes. */
export function formatBytes(n: number): string {
    if (n >= 1 << 30) return (n / (1 << 30)).toFixed(1) + " GB";
    if (n >= 1 << 20) return (n / (1 << 20)).toFixed(1) + " MB";
    if (n >= 1 << 10) return (n / (1 << 10)).toFixed(1) + " KB";
    return n + " B";
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
