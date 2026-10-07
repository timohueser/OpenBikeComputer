<script lang="ts">
    import type { HoursStatus } from '../../lib/planner/search/types';
    let { value }: { value: HoursStatus } = $props();
    const labels = { open: 'Open', closed: 'Closed', unknown: 'Hours unknown' };
</script>
<span class="status-line">
    <span class="badge" data-state={value.state}>{labels[value.state]}</span>
    {#if value.closesAt}<span>Closes at {value.closesAt}</span>{/if}
</span>
<style>
    .status-line { display: flex; align-items: center; flex-wrap: wrap; gap: 6px; margin-top: 7px; color: var(--ink-soft); font: 400 12px/1.4 var(--sans); font-variant-numeric: tabular-nums; }
    .badge { padding: 2px 8px; border-radius: 999px; color: var(--ink-soft); background: var(--parchment-2); font-weight: 600; }
    .badge[data-state="open"] { color: var(--query-place); background: color-mix(in srgb, var(--query-place) 11%, var(--panel)); }
    .badge[data-state="closed"] { color: var(--coral); background: color-mix(in srgb, var(--coral) 11%, var(--panel)); }
</style>
