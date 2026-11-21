// The time split of the ground and terrain decode workers' jobs, for the fault check of phone-diagnose.js: phone.mjs puts
// this script before the built workers. Each answer to a job carries `kit`: the clock (timeOrigin + now, the same clock as the
// page's) when the worker took the job and when it answered, the decode time that ended while the job ran, and the
// time from the last decode to the answer (the page build), and the number of WASM decodes. Jobs of one worker overlap, so a decode can count for two.
{
    const clock = () => performance.timeOrigin + performance.now();
    const taken = new Map(), decodes = [];
    // Registered before the bundle sets onmessage, so it runs first.
    self.addEventListener('message', ({ data }) => (data?.job || data?.body) && taken.set(data.id, clock()));
    const timed = (run, kind) => function (...args) {
        const start = clock();
        const done = () => decodes.push({ kind, start, end: clock() });
        const value = run.apply(this, args);
        if (value instanceof Promise) return value.finally(done);
        done();
        return value;
    };
    self.createImageBitmap = timed(self.createImageBitmap, 'bitmap');
    if (self.WebGL2RenderingContext) for (const name of ['texImage2D', 'readPixels']) WebGL2RenderingContext.prototype[name] = timed(WebGL2RenderingContext.prototype[name], name);
    // The WASM decode: wasm-bindgen calls the export through instance.exports.
    for (const name of ['instantiate', 'instantiateStreaming']) {
        const base = WebAssembly[name];
        if (!base) continue;
        WebAssembly[name] = async (...args) => {
            const result = await base(...args);
            if (!(result?.instance instanceof WebAssembly.Instance) || !result.instance.exports.decode_webp) return result;
            const exports = { ...result.instance.exports, decode_webp: timed(result.instance.exports.decode_webp, 'wasm') };
            return { module: result.module, instance: { exports } };
        };
    }
    const post = self.postMessage.bind(self);
    self.postMessage = (data, ...rest) => {
        const start = taken.get(data?.id);
        if (start !== undefined) {
            taken.delete(data.id);
            const end = clock(), mine = decodes.filter(d => d.end >= start && d.end <= end);
            const last = Math.max(start, ...mine.map(d => d.end));
            data.kit = { start, end, decode: mine.reduce((sum, d) => sum + d.end - Math.max(d.start, start), 0), decodes: mine.length, wasm: mine.filter(d => d.kind === 'wasm').length, build: end - last };
            while (decodes.length && decodes[0].end < end - 60_000) decodes.shift();
        }
        return post(data, ...rest);
    };
}
