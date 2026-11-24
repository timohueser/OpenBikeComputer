<script lang="ts">
    import { onMount } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import Callout from './Callout.svelte';
    import type { TilePlace } from '../../lib/planner/place-groups';
    import { placeCategories, poiKinds } from '../../lib/planner/poi-kinds';

    /** The places of one mark of the map, in a map popup: a click on a place opens it. */
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

<Callout label="Places here" {onClose} takeFocus={false}>
    <header><h2>Places here</h2></header>
    <ul class="rows" bind:this={list}>
        {#each rows as place (place.id)}
            <li><button type="button" class="row" onclick={() => onPick(place)}>
                <Icon path={placeCategories[place.category].icon} size={16} /><span>{place.name ?? sort(place)}</span>{#if place.name}<small>{sort(place)}</small>{/if}
            </button></li>
        {/each}
    </ul>
</Callout>
