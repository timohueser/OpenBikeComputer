// RGB565 <-> CSS-hex helpers, ported unchanged from the legacy app.js. The
// device panel shows only 64 colors (RGB222), so swatches always render the
// quantized color — the UI never promises a color the glass can't show.

/** "0xFAA0" | "FAA0" | number -> numeric RGB565 value. */
export function parseRgb565(value: string | number): number {
    if (typeof value === "number") return value & 0xffff;
    return parseInt(value.replace(/^0x/i, ""), 16) & 0xffff;
}

/** Numeric RGB565 -> canonical "0xNNNN" config form. */
export function formatRgb565(v: number): string {
    return "0x" + (v & 0xffff).toString(16).toUpperCase().padStart(4, "0");
}

/** RGB565 -> full-color CSS hex (what an ideal display would show). */
export function rgb565ToHex(value: string | number): string {
    const v = parseRgb565(value);
    const r5 = (v >> 11) & 0x1f;
    const g6 = (v >> 5) & 0x3f;
    const b5 = v & 0x1f;
    const r = Math.round((r5 * 255) / 31);
    const g = Math.round((g6 * 255) / 63);
    const b = Math.round((b5 * 255) / 31);
    return "#" + [r, g, b].map((n) => n.toString(16).padStart(2, "0")).join("");
}

/** CSS hex -> nearest RGB565 as "0xNNNN". */
export function hexToRgb565(hex: string): string {
    const r = parseInt(hex.slice(1, 3), 16);
    const g = parseInt(hex.slice(3, 5), 16);
    const b = parseInt(hex.slice(5, 7), 16);
    const v = ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3);
    return formatRgb565(v);
}

/**
 * Quantize an RGB565 value to the device's 64-color RGB222 gamut and return it
 * as a CSS hex — exactly what the panel will display (mirrors the firmware's
 * `rgb565_to_device64`).
 */
export function rgb565ToDeviceHex(value: string | number): string {
    const v = parseRgb565(value);
    const r5 = (v >> 11) & 0x1f;
    const g6 = (v >> 5) & 0x3f;
    const b5 = v & 0x1f;
    const r8 = (r5 << 3) | (r5 >> 2);
    const g8 = (g6 << 2) | (g6 >> 4);
    const b8 = (b5 << 3) | (b5 >> 2);
    const q = (x: number) => (x >> 6) * 85; // keep top 2 bits, expand (step = 85)
    return "#" + [q(r8), q(g8), q(b8)].map((n) => n.toString(16).padStart(2, "0")).join("");
}
