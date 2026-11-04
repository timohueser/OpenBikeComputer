// Scratch probe toggles of the GP8 design step, read from the page address. Not for merge.
function flags() {
    try {
        const q = new URLSearchParams(globalThis.location?.search ?? '');
        const num = (k: string, d: number) => (q.has(k) ? Number(q.get(k)) : d);
        return {
            snap: q.has('p_snap') || q.has('p_snapc'),
            snapc: q.has('p_snapc'),
            across: q.has('p_across'),
            tol0: q.has('p_tol0'),
            flat: num('p_flat', -1e9),
            noskirt: q.has('p_noskirt'),
            fillet: num('p_fillet', 0),
            flatcore: num('p_flatcore', 0),
            radius: num('p_radius', 0),
        };
    } catch {
        return { across: false, snapc: false, snap: false, tol0: false, flat: -1e9, noskirt: false, fillet: 0, flatcore: 0, radius: 0 };
    }
}
export const PROBE = flags();
