<script lang="ts">
    import type { Snippet } from 'svelte';

    let { label, children }: { label?: string; children: Snippet } = $props();
    const heading = $props.id();
</script>

<section aria-labelledby={label ? heading : undefined}>
    {#if label}<h2 id={heading}>{label}</h2>{/if}
    {@render children()}
</section>

<style>
    section {
        padding: 16px;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--panel);
    }
    h2 {
        margin: 0 0 12px;
        color: var(--ink-soft);
        font: 700 11px/1.3 var(--sans);
        letter-spacing: .06em;
        text-transform: uppercase;
    }
</style>
