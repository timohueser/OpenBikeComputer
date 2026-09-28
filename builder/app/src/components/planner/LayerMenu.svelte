<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import { categoryIds, placeCategories, type PlaceCategory } from '../../lib/planner/poi-kinds';

    let { hillshade = $bindable(), contours = $bindable(), hidden = $bindable(), highlighted = $bindable() }: {
        hillshade: boolean;
        contours: boolean;
        /** Place categories the map leaves out. */
        hidden: PlaceCategory[];
        /** Place categories shown with a ring at every zoom. */
        highlighted: PlaceCategory[];
    } = $props();

    function show(category: PlaceCategory, shown: boolean) {
        hidden = shown ? hidden.filter(c => c !== category) : [...hidden, category];
        if (!shown) highlighted = highlighted.filter(c => c !== category);
    }

    function highlight(category: PlaceCategory) {
        highlighted = highlighted.includes(category) ? highlighted.filter(c => c !== category) : [...highlighted, category];
    }
</script>

<details class="layer-menu">
    <summary aria-label="Map layers"><Icon name="layers" /></summary>
    <div class="panel">
        <strong>Map layers</strong>
        <label><input type="checkbox" bind:checked={hillshade} />Relief</label>
        <label><input type="checkbox" bind:checked={contours} />Contours</label>
        <strong class="section">Places</strong>
        <ul>
            {#each categoryIds as category (category)}
                {@const info = placeCategories[category]}
                {@const on = highlighted.includes(category)}
                <li>
                    <label>
                        <input type="checkbox" checked={!hidden.includes(category)} onchange={(event) => show(category, event.currentTarget.checked)} />
                        <Icon path={info.icon} size={15} />{info.plural}
                    </label>
                    <button type="button" class="pin" class:on aria-pressed={on} disabled={hidden.includes(category)}
                        aria-label={`Highlight ${info.plural.toLowerCase()} at every zoom`} title="Highlight at every zoom"
                        onclick={() => highlight(category)}><Icon name="pin" size={15} /></button>
                </li>
            {/each}
        </ul>
    </div>
</details>

<style>
    .layer-menu {
        position: relative;
        border-radius: 8px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
    }
    summary {
        display: grid;
        place-items: center;
        width: 36px;
        height: 36px;
        color: var(--ink);
        cursor: pointer;
        list-style: none;
    }
    summary::-webkit-details-marker {
        display: none;
    }
    summary:hover {
        color: var(--forest);
    }
    .panel {
        position: absolute;
        top: 0;
        right: 44px;
        width: 216px;
        max-height: calc(100vh - 360px);
        overflow: auto;
        padding: 12px 12px 8px 16px;
        border-radius: 8px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
        font-size: 13px;
    }
    strong {
        display: block;
        margin-bottom: 8px;
        font-weight: 600;
    }
    .section {
        margin: 12px 0 4px;
        padding-top: 12px;
        border-top: 1px solid var(--line);
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
    }
    label {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 4px 0;
    }
    li label :global(svg) {
        color: var(--ink-soft);
    }
    input {
        margin: 0;
        accent-color: var(--forest);
    }
    .pin {
        display: grid;
        place-items: center;
        width: 28px;
        height: 28px;
        padding: 0;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-faint);
        cursor: pointer;
    }
    .pin:hover:not(:disabled) {
        background: var(--parchment-2);
        color: var(--ink);
    }
    .pin.on {
        color: var(--forest);
        background: var(--parchment-2);
    }
</style>
