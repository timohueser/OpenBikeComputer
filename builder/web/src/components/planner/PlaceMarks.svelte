<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import type { GeoJSONSource, Map as MapLibre } from 'maplibre-gl';
    import type { FeatureCollection, Point } from 'geojson';
    import { FIRST_LEVEL, LAST_LEVEL, ROW_LEVEL, discMarks, rowMarks, type Mark, type TilePlace } from '../../lib/planner/place-groups';
    import { placeTile, placeTileKeys } from '../../lib/planner/place-index';
    import { placeCategories, poiKinds, type PlaceCategory } from '../../lib/planner/poi-kinds';
    import type { Coordinate } from '../../lib/planner/map-types';

    let { map, ready, theme, url, kinds, highlight, still, onOpen }: {
        map: MapLibre;
        /** The style is loaded. */
        ready: boolean;
        theme: 'light' | 'dark';
        /** The address of the place tiles. */
        url: string;
        /** The kinds that are on, with the highlighted kind. */
        kinds: PlaceCategory[];
        highlight: PlaceCategory | null;
        /** While true, the marks take no pointer events. */
        still: boolean;
        onOpen: (coordinate: Coordinate, places: TilePlace[], opener: HTMLButtonElement) => void;
    } = $props();

    const CELL = 16, LINE = 18, PER_LINE = 4, DISC = 25;
    const levelOf = () => Math.min(LAST_LEVEL, Math.floor(map.getZoom()));

    let root: HTMLDivElement;
    let level = $state(untrack(levelOf));
    let shown = $state.raw<Mark[]>([]);
    let focused = $state('');
    const quiet = $derived(kinds.filter(kind => kind !== highlight));
    const active = $derived(kinds.length > 0 && (level >= ROW_LEVEL || (!!highlight && level >= FIRST_LEVEL)));
    const ids = $derived(shown.flatMap(mark => mark.icons.map((_, index) => `${mark.key}/${index}`)));
    // All marks are one stop of the Tab key: the last focused button, or the first.
    const stop = $derived(ids.includes(focused) ? focused : ids[0]);

    /** The loaded tiles of the view with its margin, and their marks once asked for. */
    type Tile = { places: TilePlace[]; discs?: Mark[][]; rows?: Mark[][] };
    const tiles = new Map<string, Tile>();
    let wanted = new Set<string>();
    let refreshes = 0;
    const placed = new Map<HTMLElement, Mark>();
    let press: { x: number; y: number } | undefined;

    function viewBounds(): [number, number, number, number] {
        const bounds = map.getBounds(), west = bounds.getWest(), south = bounds.getSouth(), east = bounds.getEast(), north = bounds.getNorth();
        const x = (east - west) * .3, y = (north - south) * .3;
        return [west - x, south - y, east + x, north + y];
    }

    async function refresh() {
        const run = ++refreshes;
        if (!active) {
            tiles.clear();
            wanted = new Set();
            render();
            return;
        }
        const keys = await placeTileKeys(url, viewBounds()).catch(() => []);
        if (run !== refreshes) return;
        wanted = new Set(keys);
        for (const key of tiles.keys()) if (!wanted.has(key)) tiles.delete(key);
        render();
        for (const key of keys) {
            if (tiles.has(key)) continue;
            // A tile that fails shows nothing; the next refresh asks for it again.
            placeTile(url, key).then(places => {
                if (!wanted.has(key) || tiles.has(key)) return;
                tiles.set(key, { places });
                render();
            }, () => {});
        }
    }

    function render() {
        const marks: Mark[] = [];
        if (active) {
            const at = level - FIRST_LEVEL, [west, south, east, north] = viewBounds();
            for (const tile of tiles.values()) {
                if (highlight) marks.push(...(tile.discs ??= discMarks(tile.places, highlight))[at]);
                if (level >= ROW_LEVEL) marks.push(...(tile.rows ??= rowMarks(tile.places, quiet))[at]);
            }
            shown = marks.filter(mark => mark.lon >= west && mark.lon <= east && mark.lat >= south && mark.lat <= north)
                .sort((a, b) => b.lat - a.lat || a.lon - b.lon);
        } else shown = marks;
        (map.getSource('place-marks') as GeoJSONSource | undefined)?.setData(names());
    }

    const lines = (mark: Mark) => Math.ceil(mark.icons.length / PER_LINE);
    const box = (mark: Mark) => mark.disc ? [DISC, DISC] : [Math.min(mark.icons.length, PER_LINE) * CELL, lines(mark) * LINE];

    /** A clear box as large as each mark keeps the labels of the map off it; a mark of one place has its name below. */
    function names(): FeatureCollection<Point> {
        return {
            type: 'FeatureCollection', features: shown.map(mark => {
                const [width, height] = box(mark), only = mark.icons.length === 1 && mark.icons[0].places.length === 1 ? mark.icons[0].places[0] : undefined;
                return {
                    type: 'Feature', geometry: { type: 'Point', coordinates: [mark.lon, mark.lat] },
                    properties: { box: `place-box-${width}-${height}`, offset: [0, (height / 2 + 3) / 11], name: only?.name ?? '' },
                };
            }),
        };
    }

    function installNames() {
        if (map.getSource('place-marks')) return;
        const dark = theme === 'dark';
        const before = map.getLayer('contour-labels') ? 'contour-labels' : undefined;
        map.addSource('place-marks', { type: 'geojson', data: names() });
        map.addLayer({
            id: 'planner-place-names', type: 'symbol', source: 'place-marks', minzoom: ROW_LEVEL,
            layout: { 'text-field': ['get', 'name'], 'text-font': ['Noto Sans Regular'], 'text-size': 11, 'text-anchor': 'top', 'text-offset': ['get', 'offset'] },
            paint: { 'text-color': dark ? '#f2efe3' : '#1c1b14', 'text-halo-color': dark ? '#201f17' : '#ffffff', 'text-halo-width': 1.2 },
        }, before);
        // The boxes draw over the names, so the map places them first: no name, and no label of the map, covers a mark.
        map.addLayer({ id: 'planner-place-boxes', type: 'symbol', source: 'place-marks', layout: { 'icon-image': ['get', 'box'], 'icon-allow-overlap': true, 'icon-padding': 0 } }, before);
    }

    function position(element: HTMLElement, mark: Mark) {
        const { x, y } = map.project([mark.lon, mark.lat]), ratio = devicePixelRatio;
        element.style.transform = `translate(${Math.round(x * ratio) / ratio}px, ${Math.round(y * ratio) / ratio}px)`;
    }

    const track = (mark: Mark) => (element: HTMLElement) => {
        placed.set(element, mark);
        position(element, mark);
        return () => { placed.delete(element); };
    };

    function label(places: TilePlace[], category: PlaceCategory) {
        if (places.length > 1) return `${places.length} places, ${placeCategories[category].plural}`;
        const sort = poiKinds[places[0].kind].label;
        return places[0].name ? `${places[0].name}, ${sort}` : sort;
    }

    function open(event: MouseEvent) {
        const button = (event.target as Element).closest('button');
        if (!button) return;
        event.stopPropagation();
        // A keyboard click has no pointer; a pan that starts on a mark is no click.
        if (event.detail && press && Math.hypot(event.clientX - press.x, event.clientY - press.y) > 5) return;
        const [key, index] = button.dataset.id!.split('/'), mark = shown.find(mark => mark.key === key)!;
        onOpen([mark.lon, mark.lat], mark.icons[Number(index)].places, button);
    }

    function arrow(event: KeyboardEvent) {
        const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
        if (!step) return;
        event.preventDefault();
        event.stopPropagation();
        const buttons = [...root.querySelectorAll('button')];
        buttons[buttons.indexOf(document.activeElement as HTMLButtonElement) + step]?.focus();
    }

    const pressed = (event: PointerEvent) => press = { x: event.clientX, y: event.clientY };

    onMount(() => {
        // After the canvas and before each pin and the popup, so those draw over every mark.
        map.getCanvas().after(root);
        // Svelte hands `onclick` and `onkeydown` to the root of the page, after the map container has seen the event.
        root.addEventListener('click', open);
        root.addEventListener('keydown', arrow);
        root.addEventListener('pointerdown', pressed);
        const move = () => placed.forEach((mark, element) => position(element, mark));
        const zoom = () => level = levelOf();
        map.on('move', move);
        map.on('zoom', zoom);
        map.on('moveend', refresh);
        return () => {
            map.off('move', move);
            map.off('zoom', zoom);
            map.off('moveend', refresh);
            root.remove();
        };
    });

    $effect(() => {
        void [kinds, highlight];
        untrack(() => { for (const tile of tiles.values()) tile.discs = tile.rows = undefined; });
    });
    $effect(() => { void [kinds, highlight, level]; untrack(refresh); });
    $effect(() => { if (ready) untrack(installNames); });
</script>

<div class="place-marks" class:dark={theme === 'dark'} class:still bind:this={root} role="group" aria-label="Places on the map"
    onfocusin={event => focused = (event.target as HTMLElement).dataset.id ?? ''}>
    {#each shown as mark (mark.key)}
        {#if mark.disc}
            {@const { category, places } = mark.icons[0]}
            {@const id = `${mark.key}/0`}
            <button type="button" class="disc" data-id={id} tabindex={id === stop ? 0 : -1} aria-label={label(places, category)}
                {@attach track(mark)}>
                <span class="face">
                    <svg viewBox="0 0 24 24" width="13" height="13" aria-hidden="true"><path d={placeCategories[category].icon} /></svg>
                    {#if places.length > 1}<b>{places.length > 99 ? '99+' : places.length}</b>{/if}
                </span>
            </button>
        {:else}
            <div class="row" style:width={`${Math.min(mark.icons.length, PER_LINE) * CELL}px`} {@attach track(mark)}>
                {#each mark.icons as { category, places }, index (category)}
                    {@const id = `${mark.key}/${index}`}
                    <button type="button" data-id={id} tabindex={id === stop ? 0 : -1} aria-label={label(places, category)}>
                        <svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true"><path d={placeCategories[category].icon} /></svg>
                    </button>
                {/each}
            </div>
        {/if}
    {/each}
</div>

<style>
    .place-marks {
        --quiet: #59604d; --disc: var(--rust, #a4501e); --paper: #f4f2eb;
        --ease: cubic-bezier(0.23, 1, 0.32, 1);
        position: absolute; inset: 0; isolation: isolate; overflow: hidden; pointer-events: none;
    }
    .place-marks.dark { --quiet: #b9bfae; --disc: #c8642a; --paper: #181d19; }
    button { padding: 0; border: 0; background: none; font: inherit; cursor: pointer; pointer-events: auto; -webkit-tap-highlight-color: transparent; }
    .still button { pointer-events: none; }
    button:focus { outline: none; }
    svg { display: block; fill: none; stroke: currentColor; stroke-linecap: round; stroke-linejoin: round; }

    /* A row: lines of up to four cells, the block and a shorter last line centred on the spot. */
    .row { position: absolute; left: 0; top: 0; display: flex; flex-wrap: wrap; justify-content: center; translate: -50% -50%; }
    .row button { position: relative; display: grid; place-items: center; width: 16px; height: 18px; color: var(--quiet); }
    .row svg { position: relative; stroke-width: 1.9; transition: transform 120ms var(--ease); }
    /* The hover disc ends where the icon of the next cell starts, and does not grow with the icon. */
    .row button::before {
        content: ''; position: absolute; left: -1px; top: 0; width: 18px; height: 18px; border-radius: 50%;
        background: var(--panel, #fff); opacity: 0; transition: opacity 120ms var(--ease);
    }
    .row button:active svg { transform: scale(0.95); }
    .row button:focus-visible { color: var(--ink, #1c1b14); }
    .row button:focus-visible::before { opacity: 1; box-shadow: 0 0 0 2px var(--amber, #f4a81d); }

    /* A disc of the highlighted kind draws over the rows. */
    .disc { position: absolute; left: 0; top: 0; z-index: 1; translate: -50% -50%; }
    .face {
        position: relative; display: grid; place-items: center; width: 22px; height: 22px; border-radius: 50%;
        color: #fff; background: var(--disc); box-shadow: 0 0 0 1.5px var(--paper); transition: transform 120ms var(--ease);
    }
    .face svg { stroke-width: 2; }
    .face::after { content: ''; position: absolute; inset: 0; border-radius: 50%; box-shadow: 0 3px 8px rgba(28, 27, 20, .35); opacity: 0; transition: opacity 120ms var(--ease); }
    .face b {
        position: absolute; left: 14px; top: -6px; box-sizing: border-box; min-width: 14px; height: 14px; padding: 0 3px; border-radius: 7px;
        color: var(--ink, #1c1b14); background: var(--panel, #fff); font: 700 9px/14px var(--sans, sans-serif); font-variant-numeric: tabular-nums; text-align: center;
    }
    .disc:active .face { transform: scale(0.95); }
    .disc:focus-visible .face { box-shadow: 0 0 0 1.5px var(--paper), 0 0 0 3.5px var(--amber, #f4a81d); }

    @media (hover: hover) and (pointer: fine) {
        .row button:hover { color: var(--ink, #1c1b14); }
        .row button:hover::before { opacity: 1; }
        .row button:hover svg { transform: scale(1.15); }
        .row button:hover:active svg { transform: scale(0.95); }
        .disc:hover .face { transform: scale(1.1); }
        .disc:hover .face::after { opacity: 1; }
        .disc:hover:active .face { transform: scale(0.95); }
    }
    @media (prefers-reduced-motion: reduce) {
        .row svg, .face { transform: none !important; }
    }
</style>
