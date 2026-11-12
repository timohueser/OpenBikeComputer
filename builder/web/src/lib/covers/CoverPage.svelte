<!-- The page `planner.html?cover=ID`: the cover of one record of the library at the size of a card and at a large size,
     for the browser test of `Cover` and for a look at a stored cover. It is no page of the product. -->
<script lang="ts">
    import type { Library } from '../library/store';
    import Cover from './Cover.svelte';

    let { library, id }: { library: Library; id: string } = $props();
    let mark = $state<number>();
</script>

<main>
    {#each ['card', 'large'] as size (size)}
        <div class={size}><Cover {library} {id} {mark} onpoint={metres => mark = metres ?? undefined} /></div>
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
        height: 240px;
    }

    .large {
        width: 960px;
        height: 480px;
    }
</style>
