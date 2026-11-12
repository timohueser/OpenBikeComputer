// PROTOTYPE route-still (never merges). Draws the line of a route over a TrailView still on a 2D canvas,
// from the still, its depth image and the camera values of `TrailView.prototypeStill()`. It needs no
// engine and no GPU: only the frame's projection (frame/frame.ts) and the constants of the camera.

import { VFOV } from './frame/camera';
import { frameAt } from './frame/frame';

const RAD = Math.PI / 180;

/** The camera values that TrailView saves with a still: `prototypeStill()` without its depth. */
export interface StillCamera {
    viewProj: number[];
    jitter: [number, number];
    eye: number[];
    origin: [number, number];
    size: [number, number];
    output: [number, number];
    near: number;
    curvature: number;
}

/** The metres along the view axis to what each texel shows, rows from the top; Infinity is the sky. */
export interface DepthImage { width: number; height: number; distance: Float32Array }

/** A depth image as 16-bit codes of the logarithm of the distance from `min` to `max`; 65535 is the sky. */
export interface Quantised { width: number; height: number; min: number; max: number; codes: Uint16Array }

const SKY = 0xffff;

/**
 * The fly view that shows a whole route (lon, lat, metres above sea level) from above: the eye looks
 * along `heading` and `pitch`, and every point lies within `margin` of the half image.
 */
export function frameRoute(route: number[][], aspect = 16 / 9, heading = 0, pitch = -40, margin = 0.8) {
    const mean = (values: number[]) => values.reduce((a, b) => a + b, 0) / values.length;
    const frame = frameAt(mean(route.map(p => p[0])), mean(route.map(p => p[1])));
    const h = heading * RAD, p = pitch * RAD;
    const forward = [Math.sin(h) * Math.cos(p), Math.cos(h) * Math.cos(p), Math.sin(p)], right = [Math.cos(h), -Math.sin(h), 0];
    const up = [right[1] * forward[2] - right[2] * forward[1], right[2] * forward[0] - right[0] * forward[2], right[0] * forward[1] - right[1] * forward[0]];
    const dot = (a: number[], b: number[]) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    const seen = route.map(([lon, lat, z]) => {
        const at = [...frame.local(lon, lat), z];
        return [dot(at, right), dot(at, up), dot(at, forward)];
    });
    const ty = Math.tan(VFOV * RAD / 2) * margin, tx = ty * aspect;
    // The eye is `back` behind the point (x, y, 0) of the camera's axes: the least distance that holds every point, centred.
    let x = 0, y = 0, back = 0;
    for (let k = 0; k < 20; k++) {
        back = Math.max(...seen.map(s => Math.max(Math.abs(s[0] - x) / tx, Math.abs(s[1] - y) / ty) - s[2]));
        const sx = seen.map(s => (s[0] - x) / (s[2] + back)), sy = seen.map(s => (s[1] - y) / (s[2] + back)), w = mean(seen.map(s => s[2])) + back;
        x += (Math.max(...sx) + Math.min(...sx)) / 2 * w;
        y += (Math.max(...sy) + Math.min(...sy)) / 2 * w;
    }
    const eye = [0, 1, 2].map(i => x * right[i] + y * up[i] - back * forward[i]);
    const [lon, lat] = frame.lonLat(eye[0], eye[1]);
    return { lat, lon, heading, pitch, mode: 'fly' as const, height: eye[2] };
}

/** The depth image of the reversed-Z depth of the engine. */
export function depthImage(depth: Float32Array, width: number, height: number, near: number): DepthImage {
    return { width, height, distance: Float32Array.from(depth, d => (d > 0 ? near / d : Infinity)) };
}

/** Each texel from a block of `factor` × `factor`: its farthest or nearest distance, or the top left one. */
export function downsample(image: DepthImage, factor: number, mode: 'far' | 'near' | 'first'): DepthImage {
    const width = Math.ceil(image.width / factor), height = Math.ceil(image.height / factor), distance = new Float32Array(width * height);
    for (let j = 0; j < height; j++) for (let i = 0; i < width; i++) {
        let value = mode === 'far' ? 0 : Infinity;
        for (let y = j * factor; y < Math.min((j + 1) * factor, image.height); y++) for (let x = i * factor; x < Math.min((i + 1) * factor, image.width); x++) {
            const d = image.distance[y * image.width + x];
            value = mode === 'far' ? Math.max(value, d) : mode === 'near' ? Math.min(value, d) : value;
        }
        distance[j * width + i] = mode === 'first' ? image.distance[j * factor * image.width + i * factor] : value;
    }
    return { width, height, distance };
}

export function quantise({ width, height, distance }: DepthImage): Quantised {
    let min = Infinity, max = 0;
    for (const d of distance) if (Number.isFinite(d)) {
        min = Math.min(min, d);
        max = Math.max(max, d);
    }
    const scale = (SKY - 1) / Math.max(Math.log(max / min), 1e-9);
    return { width, height, min, max, codes: Uint16Array.from(distance, d => (Number.isFinite(d) ? Math.round(Math.log(d / min) * scale) : SKY)) };
}

export function dequantise({ width, height, min, max, codes }: Quantised): DepthImage {
    const scale = Math.log(max / min) / (SKY - 1);
    return { width, height, distance: Float32Array.from(codes, c => (c === SKY ? Infinity : min * Math.exp(c * scale))) };
}

/** The codes as the difference of each from the one on its left, which deflate packs smaller; `undo` takes them back. */
export function delta(codes: Uint16Array, width: number, undo = false): Uint16Array {
    const out = new Uint16Array(codes.length);
    for (let i = 0; i < codes.length; i++) {
        const left = i % width ? (undo ? out : codes)[i - 1] : 0;
        out[i] = undo ? codes[i] + left : codes[i] - left;
    }
    return out;
}

/** Deflate of the browser. */
export async function deflate(data: ArrayBufferView, undo = false): Promise<Uint8Array> {
    const bytes = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    const stream = new Blob([bytes as BlobPart]).stream().pipeThrough(undo ? new DecompressionStream('deflate') : new CompressionStream('deflate'));
    return new Uint8Array(await new Response(stream).arrayBuffer());
}

export interface Projected {
    /** x and y of each point in pixels of the still; NaN behind the eye. */
    xy: Float32Array;
    /** 1 where the point is in the image and nothing hides it. */
    visible: Uint8Array;
}

/**
 * Projects route points (lon, lat and metres above sea level, three numbers each) into the still and
 * tests each against the depth image. As the engine's ribbon, a point is `lift` above the ground and
 * moves toward the eye by `pullM` metres and `pullPixels` pixels of the still before the test.
 */
export function project(camera: StillCamera, points: ArrayLike<number>, depth: DepthImage, { lift = 0.3, pullM = 0.5, pullPixels = 4, bilinear = false } = {}): Projected {
    const frame = frameAt(...camera.origin), m = camera.viewProj, n = points.length / 3, [W, H] = camera.output, [east0, north0, z0] = camera.eye;
    const xy = new Float32Array(2 * n).fill(NaN), visible = new Uint8Array(n), pixel = 2 * Math.tan(VFOV * RAD / 2) / H;
    // The depth frame lies `jitter` render pixels off the still.
    const ju = camera.jitter[0] / camera.size[0], jv = camera.jitter[1] / camera.size[1];
    for (let i = 0; i < n; i++) {
        const [east, north] = frame.local(points[3 * i], points[3 * i + 1]);
        const x = east - east0, y = north - north0, z = points[3 * i + 2] + lift - z0 - camera.curvature * (x * x + y * y);
        const w = m[3] * x + m[7] * y + m[11] * z + m[15];
        if (!(w > camera.near)) continue;
        const u = ((m[0] * x + m[4] * y + m[8] * z + m[12]) / w + 1) / 2, v = (1 - (m[1] * x + m[5] * y + m[9] * z + m[13]) / w) / 2;
        xy[2 * i] = u * W;
        xy[2 * i + 1] = v * H;
        if (u < 0 || u >= 1 || v < 0 || v >= 1) continue;
        const reach = Math.hypot(x, y, z), pulled = w * Math.max(reach - pullM - pullPixels * pixel * reach, 0.5 * reach) / reach;
        visible[i] = pulled <= (bilinear ? sampleBilinear : sampleNearest)(depth, u + ju, v + jv) ? 1 : 0;
    }
    return { xy, visible };
}

function sampleNearest({ width, height, distance }: DepthImage, u: number, v: number): number {
    const i = Math.min(width - 1, Math.max(0, Math.floor(u * width))), j = Math.min(height - 1, Math.max(0, Math.floor(v * height)));
    return distance[j * width + i];
}

function sampleBilinear({ width, height, distance }: DepthImage, u: number, v: number): number {
    const x = Math.min(width - 1, Math.max(0, u * width - 0.5)), y = Math.min(height - 1, Math.max(0, v * height - 0.5));
    const i = Math.min(width - 2, Math.floor(x)), j = Math.min(height - 2, Math.floor(y)), fx = x - i, fy = y - j, at = j * width + i;
    return (distance[at] * (1 - fx) + distance[at + 1] * fx) * (1 - fy) + (distance[at + width] * (1 - fx) + distance[at + width + 1] * fx) * fy;
}

/**
 * Draws the still and the line over it: a bright solid line where the route is in sight, a faint dashed
 * one where something hides it, and a pointer mark at point `pointer`.
 */
export function draw(canvas: HTMLCanvasElement | OffscreenCanvas, still: CanvasImageSource, { xy, visible }: Projected, pointer?: number): void {
    const context = canvas.getContext('2d') as CanvasRenderingContext2D, unit = canvas.width / 960;
    context.drawImage(still, 0, 0, canvas.width, canvas.height);
    context.lineJoin = context.lineCap = 'round';
    const stroke = (shown: number, width: number, style: string, dash: number[]) => {
        context.beginPath();
        for (let i = 1; i < visible.length; i++) {
            if ((visible[i - 1] & visible[i]) !== shown || Number.isNaN(xy[2 * i]) || Number.isNaN(xy[2 * i - 2])) continue;
            context.moveTo(xy[2 * i - 2], xy[2 * i - 1]);
            context.lineTo(xy[2 * i], xy[2 * i + 1]);
        }
        context.lineWidth = width * unit;
        context.strokeStyle = style;
        context.setLineDash(dash.map(d => d * unit));
        context.stroke();
    };
    stroke(0, 1.5, 'rgb(255 255 255 / 0.45)', [1, 4]);
    stroke(1, 5, 'rgb(20 20 30 / 0.75)', []);
    stroke(1, 3, '#ff5a1f', []);
    if (pointer !== undefined && !Number.isNaN(xy[2 * pointer])) {
        context.setLineDash([]);
        context.beginPath();
        context.arc(xy[2 * pointer], xy[2 * pointer + 1], 6 * unit, 0, 2 * Math.PI);
        context.fillStyle = visible[pointer] ? '#ffffff' : 'rgb(255 255 255 / 0.4)';
        context.fill();
        context.lineWidth = 2 * unit;
        context.strokeStyle = '#1a1a2a';
        context.stroke();
    }
}
export { viewProjection } from './frame/camera';
