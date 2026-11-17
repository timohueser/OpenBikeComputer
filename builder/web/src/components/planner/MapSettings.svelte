<script lang="ts">
    // Prototype of issue 3555: two forms of the map settings popup, picture tiles and a list of switches.
    import Icon from '../../lib/ui/Icon.svelte';
    import type { OverlayOptions } from '../../lib/planner/route-overlays';
    import type { DataLayer } from '../../lib/planner/layers/data-layer';

    let { form, hillshade = $bindable(), contours = $bindable(), mapOverlays = $bindable(), dataLayers = [], shownLayer = $bindable(''), theme = 'light' }: {
        form: 'tiles' | 'list';
        dataLayers?: DataLayer[];
        shownLayer?: string;
        hillshade: boolean;
        contours: boolean;
        mapOverlays: OverlayOptions;
        theme?: 'light' | 'dark';
    } = $props();

    interface Item { id: string; label: string; icon: { name?: string; path?: string }; on: boolean; toggle: () => void }

    // Crops of the planner map with one layer on, made by `tiles.mjs` of the prototype.
    const pictures = import.meta.glob<string>('./tiles/*.jpg', { eager: true, query: '?url', import: 'default' });
    const picture = (id: string) => pictures[`./tiles/${id}-${theme}.jpg`];

    const networks: { id: Exclude<OverlayOptions['network'], 'none'>; label: string; icon: string }[] = [
        { id: 'cycling', label: 'Cycling', icon: 'touring' }, { id: 'hiking', label: 'Hiking', icon: 'hiking' }, { id: 'mtb', label: 'MTB', icon: 'mtb' },
    ];
    const groups = $derived<{ label: string; items: Item[] }[]>([
        { label: 'Routes, one at a time', items: networks.map(({ id, label, icon }) => ({ id, label, icon: { name: icon }, on: mapOverlays.network === id,
            toggle: () => mapOverlays = { ...mapOverlays, network: mapOverlays.network === id ? 'none' : id } })) },
        { label: 'Conditions, one at a time', items: dataLayers.map(layer => ({ id: layer.id, label: layer.label, icon: { name: layer.icon }, on: shownLayer === layer.id,
            toggle: () => shownLayer = shownLayer === layer.id ? '' : layer.id })) },
        { label: 'Map', items: [
            { id: 'relief', label: 'Relief', icon: { name: 'mountain' }, on: hillshade, toggle: () => hillshade = !hillshade },
            { id: 'contours', label: 'Contours', icon: { path: 'M3 13c0-5 4-9 10-9 5 0 8 3 8 7 0 5-4 9-10 9-5 0-8-3-8-7Zm5.5-.5c0-2.5 2-4.5 5-4.5 2.5 0 4 1.5 4 3.5 0 2.5-2 4.5-5 4.5-2.5 0-4-1.5-4-3.5Z' }, on: contours, toggle: () => contours = !contours },
            { id: 'closures', label: 'Closures', icon: { path: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18ZM7 12h10' }, on: mapOverlays.access,
                toggle: () => mapOverlays = { ...mapOverlays, access: !mapOverlays.access } },
        ] },
    ].filter(group => group.items.length));
    // What the closed button reports: the layers over the map. Relief and contours are the look of the map itself.
    const over = $derived(groups.flatMap(group => group.items).filter(item => item.on && item.id !== 'relief' && item.id !== 'contours').map(item => item.label));

    let open = $state(false);
    let root: HTMLDivElement;

    function outside(event: PointerEvent) {
        if (open && !root.contains(event.target as Node)) open = false;
    }

    function key(event: KeyboardEvent) {
        if (!open) return;
        if (event.key === 'Escape') {
            event.stopPropagation();
            open = false;
            root.querySelector<HTMLElement>('.toggle')?.focus();
            return;
        }
        // The arrow keys move through the controls as the eye does; Tab does too.
        const columns = form === 'tiles' ? 3 : 1;
        const step = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: columns, ArrowUp: -columns }[event.key];
        const controls = [...root.querySelectorAll<HTMLElement>('.panel button')];
        const index = controls.indexOf(event.target as HTMLElement);
        if (!step || index < 0 || (columns === 1 && Math.abs(step) !== 1) || !controls[index + step]) return;
        event.preventDefault();
        controls[index + step].focus();
    }
</script>

<svelte:window onpointerdown={outside} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="layer-menu" bind:this={root} onkeydown={key}>
    <button type="button" class="toggle" class:chosen={open} aria-label="Map settings" aria-describedby="map-settings-on" aria-expanded={open} aria-haspopup="dialog" onclick={() => open = !open}>
        <Icon name="layers" />
        {#if over.length}<span class="dot" aria-hidden="true"></span>{/if}
    </button>
    <span id="map-settings-on" class="sr">{over.length ? `On: ${over.join(', ')}` : 'No layers on'}</span>
    {#if open}
        <div class="panel {form}" role="dialog" aria-label="Map settings" tabindex="-1">
            {#each groups as group (group.label)}
                <div class="group" role="group" aria-label={group.label}>
                    {#each group.items as item (item.id)}
                        {#if form === 'tiles'}
                            <button type="button" class="tile" aria-pressed={item.on} onclick={item.toggle}>
                                <span class="picture">
                                    {#if picture(item.id)}<img src={picture(item.id)} alt="" draggable="false" />{/if}
                                    <span class="check" aria-hidden="true"><Icon name="check" size={11} /></span>
                                </span>
                                <span class="name">{item.label}</span>
                            </button>
                        {:else}
                            <button type="button" class="row" role="switch" aria-checked={item.on} onclick={item.toggle}>
                                <Icon {...item.icon} size={18} />
                                <span class="name">{item.label}</span>
                                <span class="track" aria-hidden="true"><span class="knob"></span></span>
                            </button>
                        {/if}
                    {/each}
                </div>
            {/each}
        </div>
    {/if}
</div>

<style>
    .layer-menu { border-radius: 8px; background: var(--panel); box-shadow: var(--planner-shadow); }
    .toggle { position: relative; display: grid; place-items: center; width: 36px; height: 36px; padding: 0; border: 0; border-radius: 8px; background: none; color: var(--ink); }
    .toggle:hover, .toggle.chosen { color: var(--link); }
    /* A layer is on: a filled dot on the corner, ringed in the panel colour so that it reads on the icon. */
    .dot { position: absolute; top: 4px; right: 4px; width: 11px; height: 11px; border: 2px solid var(--panel); border-radius: 50%; background: var(--link); }
    .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
    .panel {
        position: absolute;
        top: 0;
        right: 44px;
        width: min(280px, calc(100vw - 80px));
        max-height: calc(var(--map-height, 100vh) - 32px);
        overflow: auto;
        border-radius: 8px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
        font-size: 13px;
        transform-origin: top right;
        animation: open 140ms cubic-bezier(.2, .8, .2, 1);
    }
    @keyframes open { from { opacity: 0; transform: scale(.97); } }
    button { border: 0; background: none; color: var(--ink); }

    .tiles { display: grid; gap: 12px; padding: 12px; }
    .tiles .group { display: grid; grid-template-columns: repeat(3, 1fr); gap: 10px; }
    .tile { display: grid; gap: 5px; padding: 0; border-radius: 7px; text-align: center; }
    .picture { position: relative; display: block; aspect-ratio: 3 / 2; border-radius: 7px; background: var(--parchment-2); transition: transform 120ms ease-out; }
    .picture img { display: block; width: 100%; height: 100%; border-radius: inherit; object-fit: cover; }
    /* The edge of a picture lies over it, so a pale crop keeps its shape on the panel. */
    .picture::after { content: ''; position: absolute; inset: 0; border-radius: inherit; box-shadow: inset 0 0 0 1px var(--line-strong); }
    .tile:hover .picture::after { box-shadow: inset 0 0 0 1px var(--ink-soft); }
    .tile:active .picture { transform: scale(.97); }
    .tile .name { font-size: 12px; line-height: 16px; font-weight: 500; color: var(--ink-soft); }
    /* Lit: a ring, a check and a darker, heavier word. */
    .tile[aria-pressed="true"] .picture { outline: 2px solid var(--link); outline-offset: 1px; }
    .tile[aria-pressed="true"] .picture::after { box-shadow: none; }
    .tile[aria-pressed="true"] .name { font-weight: 650; color: var(--ink); }
    .check { position: absolute; z-index: 1; top: 4px; right: 4px; display: grid; place-items: center; width: 17px; height: 17px; border-radius: 50%; background: var(--link); color: var(--panel); opacity: 0; transform: scale(.6); transition: opacity 120ms ease-out, transform 120ms ease-out; }
    .check :global(svg) { stroke-width: 3; }
    .tile[aria-pressed="true"] .check { opacity: 1; transform: none; }

    .list { padding: 4px 14px; }
    .list .group { padding: 4px 0; }
    .list .group + .group { border-top: 1px solid var(--line); }
    .row { display: flex; align-items: center; gap: 10px; width: 100%; min-height: 36px; padding: 0; text-align: left; }
    .row :global(svg) { flex: none; color: var(--ink-soft); }
    .row .name { flex: 1; }
    .row[aria-checked="true"] .name { font-weight: 600; }
    .row[aria-checked="true"] :global(svg) { color: var(--ink); }
    .track { flex: none; width: 36px; height: 20px; padding: 2px; border-radius: 10px; background: var(--line-strong); transition: background 150ms; }
    .knob { display: block; width: 16px; height: 16px; border-radius: 50%; background: var(--panel); box-shadow: 0 1px 2px rgb(0 0 0 / 0.25); transition: transform 150ms; }
    [aria-checked="true"] .track { background: var(--forest); }
    [aria-checked="true"] .knob { transform: translateX(16px); }
    @media (prefers-reduced-motion: reduce) {
        .panel { animation: none; }
        .picture, .check, .track, .knob { transition: none; }
    }
</style>
