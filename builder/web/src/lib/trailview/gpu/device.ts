/** The features that the engine uses when the adapter has them. */
const OPTIONAL_FEATURES: GPUFeatureName[] = ['timestamp-query', 'clip-distances', 'rg11b10ufloat-renderable'];

/** The device, and the bytes that each pool holds. A lost device needs a new `Gpu`. */
export class Gpu {
    readonly pools = new Map<string, number>();
    private readonly sizes = new WeakMap<GPUBuffer | GPUTexture, number>();

    private constructor(private readonly gpuAdapter: GPUAdapter, readonly device: GPUDevice, readonly format: GPUTextureFormat) {}

    get adapter(): string {
        const { vendor, architecture, description } = this.gpuAdapter.info;
        return [vendor, architecture, description].filter(Boolean).join(' ');
    }

    /** Undefined when the browser has no WebGPU or no adapter. */
    static async request(): Promise<Gpu | undefined> {
        const adapter = await navigator.gpu?.requestAdapter({ powerPreference: 'high-performance' });
        if (!adapter) return undefined;
        const device = await adapter.requestDevice({ requiredFeatures: OPTIONAL_FEATURES.filter(feature => adapter.features.has(feature)) });
        device.addEventListener('uncapturederror', event => console.error(`WebGPU: ${event.error.message}`));
        return new Gpu(adapter, device, navigator.gpu.getPreferredCanvasFormat());
    }

    buffer(pool: string, size: number, usage: GPUBufferUsageFlags): GPUBuffer {
        return this.held(pool, this.device.createBuffer({ label: pool, size, usage }), size);
    }

    texture(pool: string, descriptor: GPUTextureDescriptor, bytes: number): GPUTexture {
        return this.held(pool, this.device.createTexture({ label: pool, ...descriptor }), bytes);
    }

    destroy(pool: string, resource: GPUBuffer | GPUTexture): void {
        this.pools.set(pool, (this.pools.get(pool) ?? 0) - (this.sizes.get(resource) ?? 0));
        resource.destroy();
    }

    private held<T extends GPUBuffer | GPUTexture>(pool: string, resource: T, bytes: number): T {
        this.sizes.set(resource, bytes);
        this.pools.set(pool, (this.pools.get(pool) ?? 0) + bytes);
        return resource;
    }
}
