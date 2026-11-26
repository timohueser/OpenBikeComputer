// The model files that the web build makes (specs/trailview-models.md): what the instances draw of a model, packed as the
// pools hold it, with the values that the instances derive from its float vertices. The build writes them (pack.ts); the
// engine reads them.
import { MeshoptDecoder } from 'meshoptimizer/decoder';
import type { Ground } from './plant-library';
import type { Season } from '../rules/regions';

/**
 * A mesh as a generator makes it: per vertex a position, a normal and a linear colour, in a model 1 tall; three indices
 * per triangle. A tree's first `trunk` vertices are its trunk, which stretches to meet its crown when the crown lifts
 * (draw.wgsl).
 */
export interface FloatMesh { position: ArrayLike<number>; normal: ArrayLike<number>; color: ArrayLike<number>; index: ArrayLike<number>; trunk?: number; glow?: ArrayLike<number>; rise?: ArrayLike<number> }

/** A mesh as the pools hold it: VERTEX_BYTES a vertex (`quantize`) at its model's extent, and three indices per triangle. */
export interface Mesh { vertices: Uint8Array<ArrayBuffer>; index: Uint32Array<ArrayBuffer> }

/**
 * What the instances read of a model's float vertices, which the build measures (pack.ts), as float32: the largest coordinate of its
 * tiers (`extent`), the radius of a sphere about (0, 0, 0.5) that holds them, the farthest that a vertex lies from the
 * axis, whether a tier has a trunk, a tree's crown base and sink table, the shadow proxy of its crown, and the impostor
 * outlines of its last tier.
 */
export interface Shape {
    extent: number;
    radius: number;
    spread: number;
    trunk: boolean;
    crown?: number;
    sinks: number[];
    proxy?: Mesh;
    outlines: Float32Array<ArrayBuffer>;
}

/** A model of a file: its mesh tiers, finest first, and its shape. */
export interface Packed { tiers: Mesh[]; shape: Shape }

/** Bytes of a vertex, the sinks of a tree and the floats of the impostor outlines (FRAMES × 4). */
export const VERTEX_BYTES = 16, SINK_SLOPES = 8, OUTLINE_FLOATS = 256;
/** The first word of a file ('TVMF'), and the version of the layout. */
export const MAGIC = 0x464d5654, VERSION = 1;

/**
 * The vertices of a mesh as the instance pipeline reads them, VERTEX_BYTES each: the position over
 * `extent` as snorm16 × 3 and, for a vertex of the trunk, the trunk's top in model units (else 0); the
 * normal as snorm8 × 3 and whether it rises at dusk; and the colour in sRGB as unorm8 × 3 with 1 − its glow at night.
 */
export function quantize(mesh: FloatMesh, extent: number): Uint8Array<ArrayBuffer> {
    const count = mesh.position.length / 3, out = new ArrayBuffer(count * VERTEX_BYTES);
    const i16 = new Int16Array(out), i8 = new Int8Array(out), u8 = new Uint8Array(out), trunk = mesh.trunk ?? 0;
    let top = 0;
    for (let k = 0; k < trunk; k++) top = Math.max(top, mesh.position[3 * k + 2]);
    const srgb = (c: number) => Math.round(255 * Math.min(1, Math.max(0, c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055)));
    for (let k = 0; k < count; k++) {
        for (let d = 0; d < 3; d++) {
            i16[8 * k + d] = Math.round(32767 * mesh.position[3 * k + d] / extent);
            i8[16 * k + 8 + d] = Math.round(127 * (mesh.normal[3 * k + d] ?? 0));
            u8[16 * k + 12 + d] = srgb(mesh.color[3 * k + d] ?? 1);
        }
        i8[16 * k + 11] = Math.round(127 * (mesh.rise?.[k] ?? 0));
        i16[8 * k + 3] = k < trunk ? Math.max(1, Math.round(32767 * Math.min(1, top))) : 0;
        u8[16 * k + 15] = 255 - Math.round(255 * (mesh.glow?.[k] ?? 0));
    }
    return u8;
}

/** A tree species: its summer crown's file and, per phase (winter first), its colour matrix (3 × 3, columns first) and the file of a crown of its own. */
export interface TreeEntry { summer: string; keepsHeading: boolean; tones: (number[] | null)[]; crowns: (string | null)[] }
/** A plant: where it grows, and per season in which it has a model the file of its SEEDS shapes. */
export interface PlantEntry { ground: Ground; seasons: Partial<Record<Season, string>> }
/** A breed: its height in metres to the top of the back, its counts of coats and poses, and the file of its models, each coat in each pose. */
export interface BreedEntry { height: number; coats: number; poses: number; file: string }

/**
 * The models of the build (`virtual:models`): what the engine knows before it fetches a file. The objects file holds the
 * rocks (`rocks` of them: slabs, then boulders) and then the models of the kinds in the order of POINT_MODELS, with
 * where each tier ends and whether the model casts a shadow.
 */
export interface Models {
    trees: Record<string, TreeEntry>;
    plants: Record<string, PlantEntry>;
    breeds: Record<string, BreedEntry>;
    objects: { file: string; rocks: number; points: { until: number[]; casts?: boolean }[] };
}

/** The models of a file; it throws where the bytes are not a model file of this version. */
export async function decodeModels(bytes: Uint8Array): Promise<Packed[]> {
    await MeshoptDecoder.ready;
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    let at = 0;
    const u32 = () => ((at += 4), view.getUint32(at - 4, true)), f32 = () => ((at += 4), view.getFloat32(at - 4, true));
    if (u32() !== MAGIC || u32() !== VERSION) throw new Error('not a model file of this version');
    const heads = Array.from({ length: u32() }, () => {
        const [extent, radius, spread, crown] = [f32(), f32(), f32(), f32()], sinks = Array.from({ length: SINK_SLOPES }, f32);
        const flags = u32(), tiers = u32(), counts = Array.from({ length: tiers + (flags & 2 ? 1 : 0) }, () => [u32(), u32()]);
        if (!tiers) throw new Error('a model of a model file has no mesh tier');
        const outlines = new Float32Array(OUTLINE_FLOATS);
        for (let k = 0; k < OUTLINE_FLOATS; k++) outlines[k] = f32();
        return { shape: { extent, radius, spread, crown: Number.isNaN(crown) ? undefined : crown, sinks, trunk: !!(flags & 1), outlines }, counts, proxy: !!(flags & 2) };
    });
    const blob = () => {
        const length = u32(), out = bytes.subarray(at, at + length);
        at += Math.ceil(length / 4) * 4;
        return out;
    };
    return heads.map(({ shape, counts, proxy }) => {
        const meshes = counts.map(([vertices, indices]): Mesh => {
            const mesh = { vertices: new Uint8Array(vertices * VERTEX_BYTES), index: new Uint32Array(indices) };
            MeshoptDecoder.decodeVertexBuffer(mesh.vertices, vertices, VERTEX_BYTES, blob());
            MeshoptDecoder.decodeIndexSequence(new Uint8Array(mesh.index.buffer), indices, 4, blob());
            return mesh;
        });
        return { tiers: proxy ? meshes.slice(0, -1) : meshes, shape: { ...shape, proxy: proxy ? meshes.at(-1) : undefined } };
    });
}
