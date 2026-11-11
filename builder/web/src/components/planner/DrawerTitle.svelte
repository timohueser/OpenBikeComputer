<script lang="ts">
    import Icon from './PlannerIcon.svelte';

    /** The title of the elevation drawer, which opens and closes it. `iconOnly` shows the chevron alone; the title stays for a screen reader and a pointer. */
    let { title, open, onToggle, iconOnly = false }: { title: string; open: boolean; onToggle: () => void; iconOnly?: boolean } = $props();
</script>

<button type="button" class:icon-only={iconOnly} aria-expanded={open} title={iconOnly ? title : open ? 'Close the drawer' : 'Open the drawer'} onclick={onToggle}>
    <Icon name="down" size={16} /><strong>{title}</strong>
</button>

<style>
    button { display: flex; align-items: center; gap: 6px; min-height: 28px; margin-left: -4px; padding: 0 6px 0 4px; border: 0; border-radius: 6px; background: none; color: var(--ink); font: 600 14px var(--sans); cursor: pointer; }
    button :global(svg) { flex: none; color: var(--ink-soft); transition: transform .18s cubic-bezier(.16, 1, .3, 1); }
    button[aria-expanded="false"] :global(svg) { transform: rotate(180deg); }
    button:hover { background: var(--parchment-2); }
    button:hover :global(svg) { color: var(--ink); }
    button:focus-visible { outline: 2px solid var(--forest); outline-offset: 2px; }
    strong { font-weight: 600; }
    /* The chevron stays at the content edge, in a target of 28 px. */
    .icon-only { justify-content: center; min-width: 28px; margin-left: -6px; padding: 0; }
    .icon-only strong { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
    @media (prefers-reduced-motion: reduce) { button :global(svg) { transition: none; } }
</style>
