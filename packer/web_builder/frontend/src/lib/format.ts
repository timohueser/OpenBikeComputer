/** Human byte sizes. Host-neutral on purpose: the build card shows a result
 *  size on every tier, and importing it from the dev host's job tracker would
 *  drag SSE polling into bundles that have no server to poll. */
export function formatBytes(n: number): string {
    if (n >= 1 << 20) return (n / (1 << 20)).toFixed(1) + " MB";
    if (n >= 1 << 10) return (n / (1 << 10)).toFixed(1) + " KB";
    return n + " B";
}
