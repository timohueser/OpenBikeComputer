// The decode check of phone-diagnose.js, a module worker: it decodes one WebP body three ways and answers the RGBA
// bytes and the time of each. `canvas` is the path of imagePixels (planner/layers/terrain.ts), `webgl` that of
// webglPixels (trailview/stream/ground-worker.ts), `wasm` the decoder of obc-webp-bridge, which phone.mjs serves.
const ways = {
    async canvas(body) {
        const bitmap = await createImageBitmap(new Blob([body]), { colorSpaceConversion: 'none', premultiplyAlpha: 'none' });
        try {
            const context = new OffscreenCanvas(bitmap.width, bitmap.height).getContext('2d', { willReadFrequently: true });
            context.drawImage(bitmap, 0, 0);
            return new Uint8Array(context.getImageData(0, 0, bitmap.width, bitmap.height).data.buffer);
        } finally {
            bitmap.close();
        }
    },
    async webgl(body) {
        const gl = new OffscreenCanvas(1, 1).getContext('webgl2');
        if (!gl) throw new Error('no WebGL 2 in a worker');
        const bitmap = await createImageBitmap(new Blob([body]), { colorSpaceConversion: 'none', premultiplyAlpha: 'none' });
        gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
        gl.pixelStorei(gl.UNPACK_COLORSPACE_CONVERSION_WEBGL, gl.NONE);
        gl.bindTexture(gl.TEXTURE_2D, gl.createTexture());
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, gl.RGBA, gl.UNSIGNED_BYTE, bitmap);
        gl.bindFramebuffer(gl.FRAMEBUFFER, gl.createFramebuffer());
        gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, gl.getParameter(gl.TEXTURE_BINDING_2D), 0);
        const data = new Uint8Array(4 * bitmap.width * bitmap.height);
        gl.readPixels(0, 0, bitmap.width, bitmap.height, gl.RGBA, gl.UNSIGNED_BYTE, data);
        bitmap.close();
        return data;
    },
    async wasm(body) {
        const { default: init, decode_webp } = await import('/phone/webp/obc_webp_bridge.js');
        await init();
        const start = performance.now(), data = decode_webp(new Uint8Array(body), new Uint32Array(1));
        return Object.assign(data, { started: start });
    },
};
self.onmessage = async ({ data: { body } }) => {
    const out = {};
    for (const [name, decode] of Object.entries(ways)) {
        const start = performance.now();
        try {
            const data = await decode(body);
            // The WASM time leaves out the load of the module.
            out[name] = { ms: performance.now() - (data.started ?? start), data };
        } catch (error) {
            out[name] = { error: String(error?.message ?? error) };
        }
    }
    self.postMessage(out, Object.values(out).flatMap(way => (way.data ? [way.data.buffer] : [])));
};
