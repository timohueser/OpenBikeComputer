import type { Gpu } from './device';

/** The bind group layouts that the passes share: make them with `gpu.once(layouts)`. */
export interface Layouts {
    /** Group 1 of the terrain: the drawn surface's pages, which drawn_height reads. */
    surface: GPUBindGroupLayout;
    /** Group 3 of the terrain: the feature mask, which drawn_height reads too. */
    features: GPUBindGroupLayout;
    /** Group 2 of the terrain, the sky and the instances: the ground pages, the look and the object shadows. */
    ground: GPUBindGroupLayout;
    /** Groups 0 and 2 of the ground compute; group 2 is that of every compute pass that reads the ground pages. */
    groundCompute: GPUBindGroupLayout[];
}

export function layouts({ device }: Gpu): Layouts {
    const all = GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT | GPUShaderStage.COMPUTE, compute = GPUShaderStage.COMPUTE, visible = GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT;
    const layout = (entries: [GPUShaderStageFlags, Omit<GPUBindGroupLayoutEntry, 'binding' | 'visibility'>][]) =>
        device.createBindGroupLayout({ entries: entries.map(([visibility, entry], binding) => ({ binding, visibility, ...entry })) });
    const read: GPUBufferBindingLayout = { type: 'read-only-storage' };
    const array = (sampleType: GPUTextureSampleType): Omit<GPUBindGroupLayoutEntry, 'binding' | 'visibility'> => ({ texture: { sampleType, viewDimension: '2d-array' } });
    // The instances' vertices read the look for the haze.
    const inputs: GPUBindGroupLayoutEntry[] = [
        { binding: 0, visibility: all, buffer: { type: 'uniform' } },
        { binding: 1, visibility: all, buffer: read },
        { binding: 2, visibility: all, ...array('uint') },
        { binding: 3, visibility: all, texture: { sampleType: 'unfilterable-float' } },
        { binding: 4, visibility: all, sampler: {} },
    ];
    return {
        surface: layout([
            [all, { buffer: { type: 'uniform' } }], [all, { buffer: read }], [all, array('uint')],
            [all, { buffer: read }], [all, array('float')], [all, { sampler: { type: 'non-filtering' } }],
        ]),
        features: layout([[all, { buffer: read }], [all, array('float')], [all, array('uint')]]),
        // The compute writes the cache: its bind group does not hold it for sampling. The producers of
        // instances read the snow in it.
        ground: device.createBindGroupLayout({ entries: [
            ...inputs, { binding: 5, visibility: GPUShaderStage.FRAGMENT | compute, ...array('float') }, { binding: 6, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: 'float' } },
            { binding: 7, visibility: all, ...array('float') },
            // The object shadows; a small instance reads them in its vertices (instances/draw.wgsl).
            { binding: 8, visibility: visible, texture: { sampleType: 'depth', viewDimension: '2d-array' } }, { binding: 9, visibility: visible, sampler: { type: 'comparison' } },
            { binding: 10, visibility: visible, buffer: { type: 'uniform' } },
        ] }),
        groundCompute: [
            device.createBindGroupLayout({ entries: [
                { binding: 0, visibility: compute, buffer: read },
                { binding: 1, visibility: compute, storageTexture: { format: 'rgba8unorm', viewDimension: '2d-array' } },
                { binding: 2, visibility: compute, ...array('float') },
                { binding: 3, visibility: compute, ...array('float') },
                { binding: 4, visibility: compute, storageTexture: { format: 'rgba8unorm', viewDimension: '2d-array' } },
            ] }),
            device.createBindGroupLayout({ entries: inputs }),
        ],
    };
}
