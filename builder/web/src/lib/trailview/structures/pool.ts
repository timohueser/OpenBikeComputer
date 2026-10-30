/** A mesh in a pool. Its bytes of vertices and of indices are multiples of 4. */
export interface Mesh { vertices: ArrayBuffer; indices: Uint16Array<ArrayBuffer> | Uint32Array<ArrayBuffer> }

/**
 * Meshes in a vertex and an index buffer of fixed size. A new mesh goes after the others. When one does
 * not fit, and the room it needs holds a mesh that the frame does not draw or that comes after it in the
 * frame's order, the pool starts again with the meshes of the frame, in their order: a mesh takes room
 * only when all before it have room, so those last in the list go first.
 */
export class Pool {
    private readonly placed = new Map<Mesh, { first: number; base: number }>();
    private vertexEnd = 0;
    private indexEnd = 0;
    /** Whether meshes that left hold room. */
    private stale = false;
    /** The meshes of the last frame that did not fit. */
    dropped = 0;

    /** `vertexBytes` is the size of a vertex. */
    constructor(private readonly device: GPUDevice, readonly vertices: GPUBuffer, readonly indices: GPUBuffer, private readonly vertexBytes: number) {}

    write(draws: Mesh[]): void {
        const placeAll = () => {
            for (const mesh of draws) if (!this.placed.has(mesh) && !this.place(mesh)) return mesh;
            return undefined;
        };
        const blocked = placeAll();
        // All meshes before the one that finds no room are in: any more hold room that is not theirs.
        if (blocked && (this.stale || this.placed.size > draws.indexOf(blocked))) {
            this.placed.clear();
            this.vertexEnd = this.indexEnd = 0;
            this.stale = false;
            placeAll();
        }
        this.dropped = draws.filter(mesh => !this.placed.has(mesh)).length;
    }

    /** Lets go of a mesh, which holds its bytes alive. Its room comes back when the pool starts again. */
    forget(mesh: Mesh): void {
        this.stale = this.placed.delete(mesh) || this.stale;
    }

    at(mesh: Mesh): { first: number; base: number } | undefined {
        return this.placed.get(mesh);
    }

    private place(mesh: Mesh): boolean {
        if (this.vertexEnd + mesh.vertices.byteLength > this.vertices.size || this.indexEnd + mesh.indices.byteLength > this.indices.size) return false;
        this.device.queue.writeBuffer(this.vertices, this.vertexEnd, mesh.vertices);
        this.device.queue.writeBuffer(this.indices, this.indexEnd, mesh.indices);
        this.placed.set(mesh, { first: this.indexEnd / mesh.indices.BYTES_PER_ELEMENT, base: this.vertexEnd / this.vertexBytes });
        this.vertexEnd += mesh.vertices.byteLength;
        this.indexEnd += mesh.indices.byteLength;
        return true;
    }
}
