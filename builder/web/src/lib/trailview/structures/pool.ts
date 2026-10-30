/** A mesh in a pool. Its bytes of vertices and of indices are multiples of 4. */
export interface Mesh { vertices: ArrayBuffer; indices: Uint16Array<ArrayBuffer> | Uint32Array<ArrayBuffer> }

/**
 * Meshes in a vertex and an index buffer of fixed size. A new mesh goes after the others; when one does
 * not fit and the pool holds meshes that the frame does not draw, the pool starts again with the meshes
 * of the frame, in their order: those last in the list are the ones that find no room.
 */
export class Pool {
    private readonly placed = new Map<Mesh, { first: number; base: number }>();
    private vertexEnd = 0;
    private indexEnd = 0;
    /** The meshes of the last frame that did not fit. */
    dropped = 0;

    /** `vertexBytes` is the size of a vertex. */
    constructor(private readonly device: GPUDevice, readonly vertices: GPUBuffer, readonly indices: GPUBuffer, private readonly vertexBytes: number) {}

    write(draws: Mesh[]): void {
        const placeAll = () => {
            for (const mesh of draws) if (!this.placed.has(mesh)) this.place(mesh);
        };
        const missing = () => draws.filter(mesh => !this.placed.has(mesh)).length;
        placeAll();
        const used = new Set(draws);
        if (missing() && [...this.placed.keys()].some(mesh => !used.has(mesh))) {
            this.placed.clear();
            this.vertexEnd = this.indexEnd = 0;
            placeAll();
        }
        this.dropped = missing();
    }

    at(mesh: Mesh): { first: number; base: number } | undefined {
        return this.placed.get(mesh);
    }

    private place(mesh: Mesh): void {
        if (this.vertexEnd + mesh.vertices.byteLength > this.vertices.size || this.indexEnd + mesh.indices.byteLength > this.indices.size) return;
        this.device.queue.writeBuffer(this.vertices, this.vertexEnd, mesh.vertices);
        this.device.queue.writeBuffer(this.indices, this.indexEnd, mesh.indices);
        this.placed.set(mesh, { first: this.indexEnd / mesh.indices.BYTES_PER_ELEMENT, base: this.vertexEnd / this.vertexBytes });
        this.vertexEnd += mesh.vertices.byteLength;
        this.indexEnd += mesh.indices.byteLength;
    }
}
