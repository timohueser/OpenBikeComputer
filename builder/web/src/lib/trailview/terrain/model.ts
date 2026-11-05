// The one road model (prototype, `ground=model`): the records of GP3's candidates per zoom-16 cell, made
// by the crate's `debug model` for the fixture and served as one file beside the feature tiles. The look
// evaluates them per fragment (model.wgsl). Not for merge.

/** Words per row of the texture that holds the records: model.wgsl's MODEL_ROW. */
export const MODEL_ROW = 4096;

function flags() {
    try {
        const q = new URLSearchParams(globalThis.location?.search ?? '');
        return { model: q.get('ground') === 'model', farRoads: q.get('farroads') !== '0', view: q.get('model') === 'labels' ? 1 : q.get('model') === 'work' ? 2 : q.get('model') === 'work3' ? 3 : q.get('model') === 'work4' ? 4 : q.get('model') === 'reach' ? 5 : 0, probe: Number(q.get('mprobe') ?? 0) };
    } catch {
        return { model: false, farRoads: true, view: 0, probe: 0 };
    }
}

/** Whether the page runs the one road model, whether it draws the roads past the masks (`farroads=0` drops them), and its debug view: 1 draws the labels flat. */
export const MODEL = flags();

let bytes: Promise<ArrayBuffer | undefined> | undefined;

/** Starts loading the records from beside the feature tiles' TileJSON. */
export function loadModel(featuresUrl: string | undefined): void {
    if (!MODEL.model || !featuresUrl || bytes) return;
    const url = new URL('/trailview-model.bin', featuresUrl);
    bytes = fetch(url).then(r => (r.ok ? r.arrayBuffer() : undefined)).catch(() => undefined);
}

/** The records' words as a texture of MODEL_ROW words per row: one word of zeros without the model. */
export async function modelTexture(device: GPUDevice): Promise<GPUTexture> {
    const body = MODEL.model ? await bytes : undefined;
    const words = body ? new Uint32Array(body) : new Uint32Array(1);
    const rows = Math.max(1, Math.ceil(words.length / MODEL_ROW));
    const padded = new Uint32Array(rows * MODEL_ROW);
    padded.set(words);
    const width = words.length > 1 ? MODEL_ROW : 1;
    const texture = device.createTexture({ label: 'model', size: [width, rows], format: 'r32uint', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
    device.queue.writeTexture({ texture }, padded, { bytesPerRow: width * 4, rowsPerImage: rows }, [width, rows]);
    return texture;
}
