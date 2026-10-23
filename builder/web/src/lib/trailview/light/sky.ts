import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';

/** The sky pass: one triangle at depth 0 behind the terrain, with the look of shade.wgsl in group 2. */
export class Sky {
    private pipeline?: GPURenderPipeline;
    private empty?: GPUBindGroup;

    async attach(gpu: Gpu, look: GPUBindGroupLayout, format: GPUTextureFormat, depthFormat: GPUTextureFormat): Promise<void> {
        const { device } = gpu, module = device.createShaderModule({ label: 'sky', code: wgsl('light/sky.wgsl') });
        const empty = device.createBindGroupLayout({ entries: [] });
        this.empty = device.createBindGroup({ layout: empty, entries: [] });
        this.pipeline = await device.createRenderPipelineAsync({
            label: 'sky', layout: device.createPipelineLayout({ bindGroupLayouts: [empty, empty, look] }),
            vertex: { module, entryPoint: 'sky_vertex' },
            fragment: { module, entryPoint: 'sky_fragment', targets: [{ format }] },
            depthStencil: { format: depthFormat, depthWriteEnabled: false, depthCompare: 'equal' },
        });
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
