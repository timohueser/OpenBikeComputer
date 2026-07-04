<script lang="ts">
    // The per-feature start-tier control: segment i lights every tier >= the
    // feature's min_lod; clicking sets the start tier.
    let {
        count,
        value,
        onchange,
    }: {
        count: number;
        value: number;
        onchange: (v: number) => void;
    } = $props();

    function hint(i: number): string {
        const where = i === 0 ? " (coarsest)" : i === count - 1 ? " (finest)" : "";
        return `Show from LOD ${i}${where} and every finer tier`;
    }
</script>

<span class="segs">
    {#each { length: count } as _, i (i)}
        <button
            type="button"
            class:on={i >= value}
            title={hint(i)}
            onclick={() => onchange(i)}
        >
            {i}
        </button>
    {/each}
</span>

<style>
    .segs {
        display: inline-flex;
        border: 1px solid var(--parchment-3);
        border-radius: 6px;
        overflow: hidden;
    }

    button {
        border: none;
        background: var(--parchment);
        color: var(--ink-faint);
        font-size: 11.5px;
        width: 21px;
        padding: 3px 0;
    }

    button + button {
        border-left: 1px solid var(--parchment-3);
    }

    button.on {
        background: var(--wood);
        color: var(--parchment);
    }
</style>
