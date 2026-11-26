<script lang="ts">
    /** The numbers of a selected part of the route, with Clear. */
    let { text, onclear }: { text: string; onclear: () => void } = $props();
</script>

<p class="part">{text}<button type="button" class="clear" onclick={onclear}>Clear</button></p>

<style>
    .part {
        display: flex;
        align-items: baseline;
        gap: 10px;
        margin: 0 0 0 4px;
        font-size: 13px;
        font-variant-numeric: tabular-nums;
    }
    .clear {
        padding: 0;
        border: 0;
        background: none;
        color: var(--ink-soft);
        font-size: 13px;
        text-decoration: underline;
    }
    .clear:hover {
        color: var(--ink);
    }
</style>
