<script lang="ts">
    /** `path` draws a 24 px stroke path instead of a named icon. */
    let { name = 'pin', path, size = 18 }: { name?: string; path?: string; size?: number } = $props();
    const paths: Record<string, string> = {
        up: 'm6 15 6-6 6 6',
        grip: 'M9 5v.1M15 5v.1M9 12v.1M15 12v.1M9 19v.1M15 19v.1',
        layers: 'm12 3 10 6-10 6L2 9l10-6Zm-10 10 10 6 10-6M2 17l10 6 10-6',
        eye: 'M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12Zm10-3a3 3 0 1 0 0 6 3 3 0 0 0 0-6',
        flag: 'M5 22V3m0 1c5-4 9 4 15 0v10c-6 4-10-4-15 0',
        pencil: 'm4 16 12-12 4 4L8 20l-5 1 1-5Zm10-10 4 4',
        pause: 'M8 5v14M16 5v14',
        search: 'M21 21l-4.5-4.5M19 10.5a8.5 8.5 0 1 1-17 0 8.5 8.5 0 0 1 17 0',
        arrow: 'M5 12h14m-6-6 6 6-6 6', back: 'M19 12H5m6-6-6 6 6 6',
        chevron: 'm9 5 7 7-7 7', down: 'm6 9 6 6 6-6', close: 'm6 6 12 12M6 18 18 6',
        pin: 'M19 10c0 5-7 11-7 11S5 15 5 10a7 7 0 1 1 14 0ZM12 7v6m-3-3h6',
        camp: 'm2 20 10-17 10 17H2Zm5 0 5-9 5 9M12 3V1',
        hotel: 'M4 21V3h16v18M2 21h20M9 21v-5h6v5M8 7h2m4 0h2M8 11h2m4 0h2',
        water: 'M12 2S5 10 5 15a7 7 0 0 0 14 0c0-5-7-13-7-13ZM8 15a4 4 0 0 0 4 4',
        check: 'm5 12 4 4L19 6', warning: 'm12 3 10 18H2L12 3Zm0 6v5m0 3v.2',
        sliders: 'M4 4v5m0 4v7M12 4v11m0 4v1M20 4v1m0 4v11M1 9h6m2 6h6m2-10h6',
        route: 'M5 3a2 2 0 1 0 0 4 2 2 0 0 0 0-4Zm14 14a2 2 0 1 0 0 4 2 2 0 0 0 0-4ZM7 5h8a4 4 0 0 1 0 8H9a4 4 0 0 0 0 8h8',
        undo: 'M4 10h10a6 6 0 0 1 0 12M4 10l5-5m-5 5 5 5',
        redo: 'M20 10H10a6 6 0 0 0 0 12m10-12-5-5m5 5-5 5',
        fit: 'M8 3H3v5m13-5h5v5M3 16v5h5m13-5v5h-5M9 12h6m-3-3v6',
        plus: 'M5 12h14m-7-7v14', minus: 'M5 12h14',
        moon: 'M20 15A9 9 0 0 1 9 4a9 9 0 1 0 11 11Z',
        sun: 'M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8ZM12 2v2m0 16v2M2 12h2m16 0h2M5 5l1 1m12 12 1 1M5 19l1-1M18 6l1-1',
        trash: 'M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 11v6m4-6v6',
        mountain: 'm2 20 7-13 4 7 3-5 6 11H2Zm4-7 3 1 2-3',
    };
</script>
<svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d={path ?? paths[name] ?? paths.pin} /></svg>
