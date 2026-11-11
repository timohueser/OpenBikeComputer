<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import { noteDistance } from '../../lib/planner/route-closures';
    import type { RouteNote } from '../../lib/planner/route-notes';

    let { notes, shown, onShow }: { notes: RouteNote[]; shown?: RouteNote; onShow: (note: RouteNote) => void } = $props();
    let open = $state(false);
    // A merged note whose start and end print the same shows its length instead.
    function place(note: RouteNote) {
        const from = note.km.toFixed(1), to = note.end?.toFixed(1);
        return `km ${from}${to && to !== from ? ` – ${to}` : note.length !== undefined ? ` · ${noteDistance(note.length)}` : ''}`;
    }
</script>

{#if notes.length}
    <div class="route-notes">
        <button type="button" class="toggle" aria-expanded={open} onclick={() => open = !open}>
            <Icon name="warning" size={16} /><span>{notes.length} {notes.length === 1 ? 'thing' : 'things'} to check</span><Icon name="down" size={14} />
        </button>
        {#if open}
            <ul>
                {#each notes as note}
                    <li><button type="button" class="note" aria-pressed={note === shown} onclick={() => onShow(note)}>
                        <Icon name={note.icon} size={15} /><span>{note.label}</span><small>{place(note)}</small>
                    </button></li>
                {/each}
            </ul>
        {/if}
    </div>
{/if}

<style>
    .route-notes { margin: 0 10px 12px; border-top: 1px solid var(--line); }
    button { display: flex; align-items: center; width: 100%; border: 0; border-radius: 6px; background: none; color: var(--ink); font-size: 13px; text-align: left; }
    button:hover, .note[aria-pressed="true"] { background: var(--parchment-2); }
    button :global(svg) { flex: none; }
    span { flex: 1; min-width: 0; }
    .toggle { gap: 8px; min-height: 36px; padding: 0 6px; font-weight: 600; }
    .toggle > :global(svg:first-child) { color: var(--forest); }
    .toggle > :global(svg:last-child) { color: var(--ink-soft); transition: transform .15s; }
    .toggle[aria-expanded="true"] > :global(svg:last-child) { transform: rotate(180deg); }
    ul { margin: 0; padding: 0; list-style: none; }
    .note { gap: 10px; min-height: 32px; padding: 7px 6px; }
    .note > :global(svg) { color: var(--ink-soft); }
    small { font-size: 12px; color: var(--ink-soft); font-variant-numeric: tabular-nums; white-space: nowrap; }
</style>
