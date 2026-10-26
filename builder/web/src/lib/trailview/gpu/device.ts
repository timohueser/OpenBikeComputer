/** The features that the engine uses when the adapter has them. */
const OPTIONAL_FEATURES: GPUFeatureName[] = ['timestamp-query', 'clip-distances', 'rg11b10ufloat-renderable'];

/** The device, and the bytes that each pool holds. A lost device needs a new `Gpu`. */
export class Gpu {
    readonly pools = new Map<string, number>();
    /** The buffers and textures made through this Gpu and not destroyed, with their pool and bytes. */
    private live = new Map<GPUBuffer | GPUTexture, [string, number]>();
    /** A scope inherits this map, so the pipelines compiled on the device are shared by every engine on it. */
    private readonly made = new Map<(gpu: Gpu) => unknown, unknown>();

    private constructor(private readonly gpuAdapter: GPUAdapter, readonly device: GPUDevice, readonly format: GPUTextureFormat) {}

    get adapter(): string {
        const { vendor, architecture, description } = this.gpuAdapter.info;
        return [vendor, architecture, description].filter(Boolean).join(' ');
    }

    /** Undefined when the browser has no WebGPU or no adapter. With no power preference the browser and the system pick the GPU. */
    static async request(): Promise<Gpu | undefined> {
        const adapter = await navigator.gpu?.requestAdapter();
        if (!adapter) return undefined;
        const device = await adapter.requestDevice({ requiredFeatures: OPTIONAL_FEATURES.filter(feature => adapter.features.has(feature)) });
        device.addEventListener('uncapturederror', event => console.error(`WebGPU: ${event.error.message}`));
        return new Gpu(adapter, device, navigator.gpu.getPreferredCanvasFormat());
    }

    /**
     * What `make` makes from the device alone, made once per device: the layouts and the pipelines,
     * which `TrailView.prepare()` can compile before an engine takes them.
     */
    once<T>(make: (gpu: Gpu) => T): T {
        if (!this.made.has(make)) this.made.set(make, make(this));
        return this.made.get(make) as T;
    }

    buffer(pool: string, size: number, usage: GPUBufferUsageFlags): GPUBuffer {
        return this.held(pool, this.device.createBuffer({ label: pool, size, usage }), size);
    }

    texture(pool: string, descriptor: GPUTextureDescriptor, bytes: number): GPUTexture {
        return this.held(pool, this.device.createTexture({ label: pool, ...descriptor }), bytes);
    }

    destroy(pool: string, resource: GPUBuffer | GPUTexture): void {
        this.pools.set(pool, (this.pools.get(pool) ?? 0) - (this.live.get(resource)?.[1] ?? 0));
        this.live.delete(resource);
        resource.destroy();
    }

    /**
     * A Gpu on the same device whose `release()` destroys only the buffers and textures made through it.
     * It inherits from this one, so it shares the device, the pools and all else.
     */
    scope(): Gpu {
        const scope = Object.create(this) as Gpu;
        scope.live = new Map();
        return scope;
    }

    /** Destroys every buffer and texture made through this Gpu. The device stays. */
    release(): void {
        for (const [resource, [pool]] of this.live) this.destroy(pool, resource);
    }

    private held<T extends GPUBuffer | GPUTexture>(pool: string, resource: T, bytes: number): T {
        this.live.set(resource, [pool, bytes]);
        this.pools.set(pool, (this.pools.get(pool) ?? 0) + bytes);
        return resource;
    }
}
