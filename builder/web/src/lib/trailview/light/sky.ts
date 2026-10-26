import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { DEPTH, sceneFormat } from '../resolve/targets';

/** The sky pass: one triangle at depth 0 behind the terrain, with the look of shade.wgsl in group 2. */
export class Sky {
    private pipeline?: GPURenderPipeline;
    private empty?: GPUBindGroup;

    async attach(gpu: Gpu): Promise<void> {
        const pipeline = await gpu.once(skyPipeline);
        this.empty = gpu.device.createBindGroup({ layout: pipeline.getBindGroupLayout(0), entries: [] });
        this.pipeline = pipeline;
    }

    draw(pass: GPURenderPassEncoder, look: GPUBindGroup): void {
        if (!this.pipeline || !this.empty) return;
        pass.setPipeline(this.pipeline);
        pass.setBindGroup(0, this.empty);
        pass.setBindGroup(1, this.empty);
        pass.setBindGroup(2, look);
        pass.draw(3);
    }
}

export function skyPipeline(gpu: Gpu): Promise<GPURenderPipeline> {
    const { device } = gpu, module = device.createShaderModule({ label: 'sky', code: wgsl('light/sky.wgsl') });
    const empty = device.createBindGroupLayout({ entries: [] });
    return device.createRenderPipelineAsync({
        label: 'sky', layout: device.createPipelineLayout({ bindGroupLayouts: [empty, empty, gpu.once(layouts).ground] }),
        vertex: { module, entryPoint: 'sky_vertex' },
        fragment: { module, entryPoint: 'sky_fragment', targets: [{ format: sceneFormat(gpu) }] },
        depthStencil: { format: DEPTH, depthWriteEnabled: false, depthCompare: 'equal' },
    });
}
