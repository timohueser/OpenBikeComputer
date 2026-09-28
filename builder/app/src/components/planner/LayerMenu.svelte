<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import { placeCategories, type PlaceCategory } from '../../lib/planner/poi-kinds';

    let { hillshade = $bindable(), contours = $bindable(), hidden = $bindable(), highlighted = $bindable() }: {
        hillshade: boolean;
        contours: boolean;
        /** Place categories the map leaves out. */
        hidden: PlaceCategory[];
        /** Place categories shown with a ring at every zoom. */
        highlighted: PlaceCategory[];
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
        open = false;
        root.querySelector<HTMLElement>('.toggle')?.focus();
    }
</script>

<svelte:window onpointerdown={outside} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="layer-menu" bind:this={root} onkeydown={key}>
    <button type="button" class="toggle" class:chosen={open} aria-label="Map layers" aria-expanded={open} aria-haspopup="true" onclick={() => open = !open}><Icon name="layers" /></button>
    {#if open}
        <div class="panel">
            <strong>Map layers</strong>
            <label><input type="checkbox" bind:checked={hillshade} />Relief</label>
            <label><input type="checkbox" bind:checked={contours} />Contours</label>
            {#each groups as group (group.title)}
                <strong class="section">{group.title}</strong>
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
        width: 224px;
        max-height: calc(var(--map-height, 100vh) - 32px);
        overflow: auto;
        padding: 12px 8px 8px 16px;
        border-radius: 8px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
        font-size: 13px;
    }
    strong {
        display: block;
        margin-bottom: 4px;
        font-weight: 600;
    }
    .section {
        margin: 8px 0 0;
        padding-top: 8px;
        border-top: 1px solid var(--line);
        font-size: 11px;
        color: var(--ink-soft);
    }
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
