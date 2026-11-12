<!-- The page `planner.html?cover=ID`: the cover of one record of the library in a home card, which shows the map picture
     when the record has no cover, and at a large size, for the browser test of `Cover` and for a look at a stored cover.
     It is no page of the product. -->
<script lang="ts">
    import type { Library } from '../library/store';
    import Cover from './Cover.svelte';

    let { library, id }: { library: Library; id: string } = $props();
    let mark = $state<number>();
</script>

<main>
    {#each [['card', 'card'], ['large', 'page']] as const as [size, place] (size)}
        <div class={size}><Cover {library} {id} {mark} {place} onpoint={metres => mark = metres ?? undefined} /></div>
    {/each}
</main>

<style>
    main {
        display: flex;
        flex-wrap: wrap;
        align-items: flex-start;
        gap: 16px;
        padding: 16px;
    }

    .card {
        width: 320px;
        height: 150px;
    }

    .large {
        width: 960px;
        height: 480px;
    }
</style>
