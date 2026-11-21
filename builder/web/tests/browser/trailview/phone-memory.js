// Prototype #3765, the memory probe of phone.mjs (/phone/memory): it allocates in steps of 64 MiB until the system stops the
// page or its GPU process, and posts the total after each step, so the last post is the limit. ?kind=js fills ArrayBuffers in
// the web content process; ?kind=gpu fills GPU buffers, which the GPU process holds. ?host=NAME labels the session.
(() => {
    const STEP = 64 * 2 ** 20, MAX = 12 * 2 ** 30;
    const query = new URLSearchParams(location.search), kind = query.get('kind');
    const session = `memory-${query.get('host') ?? 'web'}-${kind}-${new Date().toISOString().slice(0, 19).replace(/[-:T]/g, '')}`;
    const post = (key, value) => fetch('/result', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ session, key, value }) }).catch(() => {});
    const line = text => document.body.append(Object.assign(document.createElement('p'), { textContent: text }));

    addEventListener('DOMContentLoaded', async () => {
        document.body.style.cssText = 'margin:0;padding:24px 16px;font:20px/1.4 system-ui,sans-serif;background:#fff;color:#111';
        if (kind !== 'js' && kind !== 'gpu') {
            for (const k of ['js', 'gpu']) line(`<a href="?kind=${k}">${k}</a>`);
            document.body.innerHTML = document.body.textContent;
            return;
        }
        line(`Memory probe (${kind}). The page or the view stops at the limit.`);
        const shown = document.body.appendChild(document.createElement('p'));
        await post('device', { userAgent: navigator.userAgent, kind, step: STEP });
        const keep = [];
        let device;
        if (kind === 'gpu') {
            device = await (await navigator.gpu.requestAdapter()).requestDevice({});
            device.lost.then(info => post('event', { lost: info.reason, message: info.message, mib: (keep.length * STEP) / 2 ** 20 }));
        }
        for (let total = STEP; total <= MAX; total += STEP) {
            if (kind === 'js') {
                const bytes = new Uint8Array(STEP);
                for (let i = 0; i < STEP; i += 4096) bytes[i] = 1;
                keep.push(bytes);
            } else {
                device.pushErrorScope('out-of-memory');
                const buffer = device.createBuffer({ size: STEP, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST, mappedAtCreation: true });
                new Uint8Array(buffer.getMappedRange()).fill(1);
                buffer.unmap();
                const error = await device.popErrorScope();
                if (error) return void (await post('event', { error: error.message, mib: total / 2 ** 20 }), (shown.textContent = `Refused at ${total / 2 ** 20} MiB`));
                keep.push(buffer);
            }
            shown.textContent = `${total / 2 ** 20} MiB`;
            await post('event', { mib: total / 2 ** 20 });
        }
        await post('event', { done: MAX / 2 ** 20 });
    });
})();
