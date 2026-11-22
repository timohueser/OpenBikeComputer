<script lang="ts">
    import Icon from '../../lib/ui/Icon.svelte';
    import Menu from '../../lib/ui/Menu.svelte';
    import { planTitle, type Trip } from '../../lib/planner/editor';

    let { trip, name, stored, changed, canSave, canUndo, canRedo, onUndo, onRedo, onSave, onRename, more, ready }: {
        trip: Trip;
        name: string;
        /** The route has a record in the library: its title renames it, and with no changes it is "Saved". */
        stored: boolean;
        changed: boolean;
        canSave: boolean;
        ready: boolean;
        canUndo: boolean;
        canRedo: boolean;
        onUndo: () => void;
        onRedo: () => void;
        onSave: () => void;
        onRename: () => void;
        /** The items of the "More" menu. */
        more: { label: string; disabled?: boolean; rule?: boolean; run: () => void }[];
    } = $props();

    const title = $derived(name || planTitle(trip));
    const saved = $derived(stored && !changed);
</script>

<div class="trip-bar" inert={!ready}>
    <h1>{#if stored}<button type="button" class="rename" {title} aria-haspopup="dialog" onclick={onRename}>{title}</button>{:else}<span {title}>{title}</span>{/if}</h1>
    <div class="actions">
        <button type="button" class="icon" disabled={!canUndo} onclick={onUndo} aria-label="Undo" title="Undo"><Icon name="undo" /></button>
        <button type="button" class="icon" disabled={!canRedo} onclick={onRedo} aria-label="Redo" title="Redo"><Icon name="redo" /></button>
        <div class="save">
            <!-- One button that stays in the page, so it keeps the focus after a save. -->
            <button type="button" class={saved ? 'saved' : 'planner-action primary'} aria-disabled={!ready || !canSave}
                onclick={() => { if (ready && canSave) onSave(); }}>{#if saved}<Icon name="check" size={15} />Saved{:else}Save{/if}</button>
            <Menu label="More" items={more} buttonClass="planner-action quiet" />
        </div>
    </div>
</div>

<style>
    .trip-bar {
        display: grid;
        grid-template-columns: minmax(0, 1fr) auto;
        align-items: center;
        gap: 12px;
        min-height: 56px;
        flex: none;
        padding: 8px 16px;
        border-bottom: 1px solid var(--line);
        background: var(--panel);
    }
    h1 {
        display: flex;
        min-width: 0;
        margin: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        font: 700 17px var(--sans);
    }
    h1 > * {
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
    }
    .rename {
        margin: 0 -6px;
        padding: 2px 6px;
        white-space: nowrap;
        border: 0;
        border-radius: 6px;
        background: none;
        color: inherit;
        font: inherit;
        text-align: left;
    }
    .rename:hover { background: var(--parchment-2); }
    .save {
        display: flex;
        align-items: center;
        gap: 4px;
        margin-left: 8px;
    }
    .saved {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        min-height: 32px;
        padding: 0 8px;
        border: 0;
        background: none;
        color: var(--ink-soft);
        font: inherit;
        font-weight: 600;
        cursor: default;
    }
    .primary[aria-disabled="true"] {
        opacity: 0.45;
        cursor: default;
        pointer-events: none;
    }
    .actions {
        display: flex;
        align-items: center;
        gap: 4px;
    }
    .actions :global(button) { white-space: nowrap; }
    .icon {
        display: grid;
        place-items: center;
        width: 30px;
        height: 30px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink);
    }
    .icon:hover:not(:disabled) {
        background: var(--parchment-2);
    }
</style>
