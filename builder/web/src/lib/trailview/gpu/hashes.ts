// The determinism check's reads (tests/browser/trailview/determinism.mjs): a hash of named GPU regions,
// so that two runs of one view compare their drawn state region by region.

/** A texture rectangle of one layer, or a buffer range; regions of the same name hash together, in order. */
export type Region = { name: string } & (
    | { texture: GPUTexture; origin: [number, number, number]; size: [number, number]; texelBytes: number }
    | { buffer: GPUBuffer; offset: number; bytes: number });

/** Bytes that one staging buffer takes at most. */
const STAGING = 32 * 2 ** 20;

const bytesOf = (region: Region) => ('texture' in region ? align(region.size[0] * region.texelBytes) * region.size[1] : region.bytes);
const align = (bytes: number) => Math.ceil(bytes / 256) * 256;

/** Per name, two 32-bit FNV-1a-like hashes of the regions' bytes (a texture's rows without their padding), as 16 hex digits. */
export async function hashRegions(device: GPUDevice, regions: Region[]): Promise<Record<string, string>> {
    const state = new Map<string, [number, number]>();
    for (let first = 0; first < regions.length;) {
        let bytes = 0, last = first;
        while (last < regions.length && (last === first || bytes + bytesOf(regions[last]) <= STAGING)) bytes += bytesOf(regions[last++]);
        const staging = device.createBuffer({ size: bytes, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST });
        const encoder = device.createCommandEncoder();
        let at = 0;
        for (const region of regions.slice(first, last)) {
            if ('texture' in region) encoder.copyTextureToBuffer({ texture: region.texture, origin: region.origin }, { buffer: staging, offset: at, bytesPerRow: align(region.size[0] * region.texelBytes) }, [...region.size, 1]);
            else encoder.copyBufferToBuffer(region.buffer, region.offset, staging, at, region.bytes);
            at += bytesOf(region);
        }
        device.queue.submit([encoder.finish()]);
        await staging.mapAsync(GPUMapMode.READ);
        const words = new Uint32Array(staging.getMappedRange());
        at = 0;
        for (const region of regions.slice(first, last)) {
            const h = state.get(region.name) ?? start();
            const [row, rows] = 'texture' in region ? [align(region.size[0] * region.texelBytes), region.size[1]] : [region.bytes, 1];
            const used = 'texture' in region ? region.size[0] * region.texelBytes : region.bytes;
            for (let r = 0; r < rows; r++) for (let w = (at + r * row) / 4, end = w + used / 4; w < end; w++) mix(h, words[w]);
            state.set(region.name, h);
            at += bytesOf(region);
        }
        staging.unmap();
        staging.destroy();
        first = last;
    }
    return Object.fromEntries([...state].map(([name, h]) => [name, hex(h)]));
}

/** The same hash of the bytes of arrays on the CPU, in order. */
export function hashBytes(arrays: ArrayBufferView[]): string {
    const h = start();
    for (const array of arrays) for (const byte of new Uint8Array(array.buffer, array.byteOffset, array.byteLength)) mix(h, byte);
    return hex(h);
}

const start = (): [number, number] => [0x811c9dc5, 0x9e3779b9];

function mix(h: [number, number], word: number): void {
    h[0] = Math.imul(h[0] ^ word, 16777619);
    h[1] = (Math.imul(h[1] ^ word, 0x01000193) + 0x7f4a7c15) | 0;
}

const hex = ([a, b]: [number, number]) => (a >>> 0).toString(16).padStart(8, '0') + (b >>> 0).toString(16).padStart(8, '0');
