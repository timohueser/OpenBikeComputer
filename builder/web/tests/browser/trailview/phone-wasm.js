// Prototype #3765: the kit puts this before every TrailView worker. Every 5 s it posts the bytes of the worker's WASM memories
// to the session `workers-<time>` of the kit, so the memory outside the GPU pools can be split by worker.
(() => {
    const memories = new Set(), name = String(self.location?.pathname ?? '').split('/').pop().replace(/-[\w]+\.js$/, '');
    const keep = result => {
        const exports = (result.instance ?? result).exports ?? {};
        for (const value of Object.values(exports)) if (value instanceof WebAssembly.Memory) memories.add(value);
        return result;
    };
    for (const method of ['instantiate', 'instantiateStreaming']) {
        const base = WebAssembly[method];
        if (base) WebAssembly[method] = (...args) => base.apply(WebAssembly, args).then(keep);
    }
    const Base = WebAssembly.Instance;
    WebAssembly.Instance = function (...args) { return keep(new Base(...args)); };
    WebAssembly.Instance.prototype = Base.prototype;
    const session = `workers-${new Date().toISOString().slice(0, 13).replace(/[-T]/g, '')}`;
    setInterval(() => {
        let bytes = 0;
        for (const memory of memories) bytes += memory.buffer.byteLength;
        const value = { worker: name, wasmMiB: Math.round(bytes / 2 ** 20), memories: memories.size };
        fetch('/result', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ session, key: 'event', value }) }).catch(() => {});
    }, 5000);
})();
