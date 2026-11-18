<script lang="ts">
    import { onMount } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import type { TilePlace } from '../../lib/planner/place-groups';
    import { placeCategories, poiKinds } from '../../lib/planner/poi-kinds';

    let { places, onPick, onClose }: { places: TilePlace[]; onPick: (place: TilePlace) => void; onClose: () => void } = $props();

    const sort = (place: TilePlace) => poiKinds[place.kind].label;
    const rows = $derived([...places].sort((a, b) => (a.name ?? sort(a)).localeCompare(b.name ?? sort(b))));
    let list: HTMLUListElement;

    onMount(() => {
        // The map shows the popup after the list mounts in its hidden storage.
        const frame = requestAnimationFrame(() => list.querySelector('button')?.focus());
        return () => cancelAnimationFrame(frame);
    });
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="place-list" role="dialog" aria-label="Places here" tabindex="-1"
    onkeydown={event => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onClose(); } }}>
    <div class="head"><strong>Places here</strong><button type="button" class="close" aria-label="Close" onclick={onClose}><Icon name="close" size={15} /></button></div>
    <ul bind:this={list}>
        {#each rows as place (place.id)}
            <li><button type="button" onclick={() => onPick(place)}>
                <Icon path={placeCategories[place.category].icon} size={15} /><span class="name">{place.name ?? sort(place)}</span>{#if place.name}<span class="sort">{sort(place)}</span>{/if}
            </button></li>
        {/each}
    </ul>
</div>

<style>
    .place-list { display: flex; flex-direction: column; box-sizing: border-box; width: 264px; max-width: var(--callout-width); max-height: var(--callout-room); padding: 6px; color: var(--ink); font: 400 13px var(--sans); }
    .head { display: flex; align-items: center; justify-content: space-between; padding: 0 0 4px 8px; }
    strong { font: 600 13px var(--sans); }
    .close { display: grid; place-items: center; width: 28px; height: 28px; padding: 0; border: 0; border-radius: 6px; background: none; color: var(--ink-soft); cursor: pointer; }
    .close:hover { background: var(--parchment-2); color: var(--ink); }
    ul { min-height: 0; margin: 0; padding: 0; overflow-y: auto; overscroll-behavior: contain; list-style: none; }
    li button { display: flex; align-items: center; gap: 8px; width: 100%; min-height: 32px; padding: 0 8px; border: 0; border-radius: 6px; background: none; color: inherit; font: inherit; text-align: left; cursor: pointer; }
    li button:hover { background: var(--parchment-2); }
    li button:focus-visible { outline: 2px solid var(--amber); outline-offset: -2px; }
    li button :global(svg) { flex: none; color: var(--ink-soft); }
    .name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .sort { flex: none; margin-left: auto; color: var(--ink-soft); font-size: 12px; }
</style>
