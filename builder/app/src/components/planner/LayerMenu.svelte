<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import Segmented from './Segmented.svelte';
    import { placeCategories, type PlaceCategory } from '../../lib/planner/poi-kinds';
    import { networkLevels, type OverlayOptions } from '../../lib/planner/route-overlays';

    let { autoCenter = $bindable(false), hillshade = $bindable(), contours = $bindable(), hidden = $bindable(), highlighted = $bindable(), mapOverlays = $bindable(), theme = 'light', walking = false }: {
        autoCenter?: boolean;
        walking?: boolean;
        hillshade: boolean;
        contours: boolean;
        /** Place categories the map leaves out. */
        hidden: PlaceCategory[];
        /** Place categories shown with a ring at every zoom. */
        highlighted: PlaceCategory[];
        mapOverlays: OverlayOptions;
        theme?: 'light' | 'dark';
    } = $props();

    const groups: { title: string; categories: PlaceCategory[] }[] = [
        { title: 'Sleep', categories: ['hotel', 'camp', 'shelter'] },
        { title: 'Eat & drink', categories: ['shop', 'food'] },
        { title: 'Water', categories: ['water', 'toilets'] },
        { title: 'Fix & go', categories: ['bike', 'pharmacy', 'station'] },
        { title: 'See', categories: ['viewpoint', 'peak'] },
    ];

    let open = $state(false);
    let root: HTMLDivElement;
    const networks: { value: OverlayOptions['network']; label: string }[] = [
        { value: 'none', label: 'Off' }, { value: 'cycling', label: 'Cycling' }, { value: 'hiking', label: 'Hiking' },
    ];

    function show(category: PlaceCategory, shown: boolean) {
        hidden = shown ? hidden.filter(c => c !== category) : [...hidden, category];
        if (!shown) highlighted = highlighted.filter(c => c !== category);
    }

    function highlight(category: PlaceCategory) {
        highlighted = highlighted.includes(category) ? highlighted.filter(c => c !== category) : [...highlighted, category];
    }

    function outside(event: PointerEvent) {
        if (open && !root.contains(event.target as Node)) open = false;
    }

    function key(event: KeyboardEvent) {
        if (event.key !== 'Escape' || !open) return;
        event.stopPropagation();
        close();
    }

    function close() {
        open = false;
        root.querySelector<HTMLElement>('.toggle')?.focus();
    }
</script>

<svelte:window onpointerdown={outside} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="layer-menu" bind:this={root} onkeydown={key}>
    <button type="button" class="toggle" class:chosen={open} aria-label="Map settings" aria-expanded={open} aria-haspopup="dialog" onclick={() => open = !open}><Icon name="layers" /></button>
    {#if open}
        <div class="panel" role="dialog" aria-label="Map settings" tabindex="-1">
            <div class="heading"><strong>Map settings</strong><button class="close" aria-label="Close map settings" onclick={close}><Icon name="close" size={16} /></button></div>
            <section aria-label="Route networks">
                <Segmented label="Route network" options={networks} value={mapOverlays.network} onChange={network => mapOverlays = { ...mapOverlays, network }} />
            {#if mapOverlays.network !== 'none'}
                <ul class="legend" aria-label="Route network colors">
                    {#each networkLevels as level (level.rank)}<li title={level.label}><span class="sample" style:color={theme === 'dark' ? level.dark : level.color}></span>{level.rank === 3 ? 'National / intl.' : level.rank === 0 ? 'Unspecified' : level.label}</li>{/each}
                </ul>
            {/if}
                <p>Right-click or long-press a route for details.</p>
            </section>
            <section class="access-section" aria-label="Access markings">
                <label><input type="checkbox" bind:checked={mapOverlays.access} />Closures & access<span class="access-symbol" aria-hidden="true"><Icon path="M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18ZM7 12h10" size={17} /></span></label>
                {#if mapOverlays.access}<p>{walking ? 'Walking access.' : 'Bike access: a walking symbol means dismount and push.'} Click access symbols for rules; closure reports are not live.</p>{/if}
            </section>
            <section class="access-section" aria-label="Planning controls"><label><input type="checkbox" bind:checked={autoCenter} />Center on added points</label></section>
            <details>
                <summary><Icon name="mountain" size={17} /><strong>Terrain</strong><span class="detail-value">{[hillshade && 'Relief', contours && 'Contours'].filter(Boolean).join(' · ') || 'Off'}</span><Icon name="chevron" size={14} /></summary>
                <div class="terrain"><label><input type="checkbox" bind:checked={hillshade} />Relief</label><label><input type="checkbox" bind:checked={contours} />Contours</label></div>
            </details>
            <details>
                <summary><Icon name="pin" size={17} /><strong>Places</strong><span class="detail-value">{Object.keys(placeCategories).length - hidden.length} shown</span><Icon name="chevron" size={14} /></summary>
                <p>Use the ring to highlight a place type at every zoom.</p>
                {#each groups as group (group.title)}
                    <h3>{group.title}</h3>
                    <ul>
                        {#each group.categories as category (category)}
                            {@const info = placeCategories[category]}
                            {@const on = highlighted.includes(category)}
                            <li>
                                <label>
                                    <input type="checkbox" checked={!hidden.includes(category)} onchange={(event) => show(category, event.currentTarget.checked)} />
                                    <Icon path={info.icon} size={15} />{info.plural}
                                </label>
                                <button type="button" class="ring" class:on aria-pressed={on} disabled={hidden.includes(category)}
                                    aria-label={`Highlight ${info.plural.toLowerCase()} at every zoom`} title="Highlight at every zoom"
                                    onclick={() => highlight(category)}><span></span></button>
                            </li>
                        {/each}
                    </ul>
                {/each}
            </details>
        </div>
    {/if}
</div>

<style>
    .layer-menu {
        border-radius: 8px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
    }
    .toggle {
        display: grid;
        place-items: center;
        width: 36px;
        height: 36px;
        padding: 0;
        border: 0;
        border-radius: 8px;
        background: none;
        color: var(--ink);
        cursor: pointer;
    }
    .toggle:hover,
    .toggle.chosen {
        color: var(--link);
    }
    /* Positioned against the controls column, so it starts level with the first control and stays inside the map. */
    .panel {
        position: absolute;
        top: 0;
        right: 44px;
        width: min(280px, calc(100vw - 80px));
        box-sizing: border-box;
        max-height: calc(var(--map-height, 100vh) - 32px);
        overflow: auto;
        padding: 10px 14px 4px;
        border-radius: 8px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
        font-size: 13px;
    }
    strong { font-weight: 600; }
    .heading { display: flex; align-items: center; justify-content: space-between; margin-bottom: 8px; font-size: 15px; }
    .close { display: grid; place-items: center; width: 28px; height: 28px; border: 0; border-radius: 5px; background: transparent; color: var(--ink-soft); cursor: pointer; }
    .close:hover { background: var(--parchment-2); color: var(--ink); }
    h3 { margin: 12px 0 8px; font-size: 12px; font-weight: 600; color: var(--ink-soft); }
    section { padding-bottom: 8px; }
    .access-section { padding: 6px 0; border-top: 1px solid var(--line); }
    .access-symbol { margin-left: auto; display: flex; color: var(--ink-soft); }
    details {
        border-top: 1px solid var(--line);
    }
    summary { display: flex; align-items: center; gap: 8px; min-height: 38px; cursor: pointer; list-style: none; }
    summary::-webkit-details-marker { display: none; }
    summary:hover { color: var(--link); }
    summary :global(svg) { flex-shrink: 0; color: var(--ink-soft); }
    details[open] summary :global(svg:last-child) { transform: rotate(90deg); }
    details[open] { padding-bottom: 8px; }
    .detail-value { margin-left: auto; font-size: 11px; color: var(--ink-soft); }
    .terrain { display: flex; gap: 12px; }
    .sample { display: inline-block; flex: 0 0 14px; width: 14px; border-top: 3px solid currentColor; }
    .legend { display: grid; grid-template-columns: 1fr 1fr; column-gap: 8px; margin: 8px 0 0; font-size: 12px; color: var(--ink-soft); }
    .legend li { justify-content: flex-start; min-height: 22px; }
    p { margin: 2px 0 8px; font-size: 12px; line-height: 1.45; color: var(--ink-soft); }
    ul {
        margin: 0;
        padding: 0;
        list-style: none;
    }
    li {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
    }
    label {
        display: flex;
        flex: 1;
        align-items: center;
        gap: 8px;
        min-height: 32px;
        color: var(--ink);
        cursor: pointer;
    }
    li label :global(svg) {
        color: var(--ink-soft);
    }
    input {
        width: 18px;
        height: 18px;
        margin: 0;
        accent-color: var(--link);
        cursor: pointer;
    }
    .ring {
        display: grid;
        place-items: center;
        width: 28px;
        height: 28px;
        padding: 0;
        border: 0;
        border-radius: 6px;
        background: none;
        cursor: pointer;
    }
    .ring span {
        width: 14px;
        height: 14px;
        border: 2px solid var(--line-strong);
        border-radius: 50%;
    }
    .ring:hover:not(:disabled) {
        background: var(--parchment-2);
    }
    .ring:hover:not(:disabled) span {
        border-color: var(--ink-soft);
    }
    .ring.on span {
        border-width: 3px;
        border-color: var(--amber);
    }
    .ring:disabled {
        cursor: default;
    }
</style>
