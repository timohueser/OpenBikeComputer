<script lang="ts">
    import { onMount, tick } from 'svelte';
    import { planTitle } from '../../lib/planner/versions';
    import type { Plan } from '../../lib/planner/library';
    import Icon from './PlannerIcon.svelte';

    let { plans, activeId, busy, error, onClose, onOpen, onRename, onDuplicate, onDelete, onDownload, onImport }: {
        plans: Plan[]; activeId: string; busy: boolean; error: string;
        onClose: () => void; onOpen: (plan: Plan) => void; onRename: (plan: Plan, name: string) => Promise<void>;
        onDuplicate: (plan: Plan) => void; onDelete: (plan: Plan) => void; onDownload: (plan: Plan) => void;
        onImport: (file: File) => void;
    } = $props();

    let renaming = $state<string | null>(null);
    let name = $state('');
    let panel: HTMLElement;
    let picker: HTMLInputElement;
    let closer: HTMLButtonElement;

    onMount(() => {
        const opener = document.activeElement as HTMLElement | null;
        closer.focus();
        return () => opener?.focus();
    });

    function key(event: KeyboardEvent) {
        if (event.key === 'Escape') {
            event.stopPropagation();
            if (renaming) renaming = null;
            else onClose();
        }
    }

    async function rename(plan: Plan) {
        if (!name.trim()) return;
        await onRename(plan, name.trim());
        if (!error) renaming = null;
    }
</script>

<svelte:window onpointerdown={event => { if (!panel.contains(event.target as Node) && !(event.target as Element).closest('button')?.textContent?.includes('My plans')) onClose(); }} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<section class="library" aria-label="My plans" bind:this={panel} onkeydown={key}>
    <header>
        <h2>My plans</h2>
        <button type="button" class="icon" aria-label="Close My plans" bind:this={closer} onclick={onClose}><Icon name="close" /></button>
    </header>
    <p class="storage">Saved in this browser. Download a plan to keep a backup or move it to another device.</p>
    <input class="file" type="file" accept=".obcplan,application/json" aria-label="Import plan file" bind:this={picker} onchange={() => {
        const file = picker.files?.[0];
        if (file) onImport(file);
        picker.value = '';
    }} />
    <button type="button" class="planner-action" disabled={busy} onclick={() => picker.click()}>Import plan</button>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#if !plans.length}
        <p class="empty">Your plans appear here when you choose a point on the map.</p>
    {:else}
        <ul>
            {#each plans as plan (plan.id)}
                <li>
                    {#if renaming === plan.id}
                        <form onsubmit={event => { event.preventDefault(); void rename(plan); }}>
                            <label>Plan name<input aria-label="Plan name" maxlength="120" bind:value={name} disabled={busy} /></label>
                            <button type="submit" class="planner-action" disabled={busy || !name.trim()}>Rename</button>
                            <button type="button" class="planner-action quiet" onclick={() => renaming = null}>Cancel</button>
                        </form>
                    {:else}
                        <button type="button" class="open" disabled={busy} aria-current={plan.id === activeId ? 'true' : undefined} onclick={() => onOpen(plan)}>
                            <strong>{plan.name || planTitle(plan.trip)}</strong>
                            <span>{plan.summary}</span>
                            <small>{plan.id === activeId ? 'Open · ' : ''}Edited {new Date(plan.updatedAt).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })}</small>
                        </button>
                        <div class="actions">
                            <button type="button" class="planner-action quiet" disabled={busy} onclick={() => { renaming = plan.id; name = plan.name || planTitle(plan.trip); void tick().then(() => panel.querySelector<HTMLInputElement>('[aria-label="Plan name"]')?.select()); }}>Rename</button>
                            <button type="button" class="planner-action quiet" disabled={busy} onclick={() => onDuplicate(plan)}>Duplicate</button>
                            <button type="button" class="planner-action quiet" onclick={() => onDownload(plan)}>Download</button>
                            <button type="button" class="planner-action quiet" disabled={busy} onclick={() => onDelete(plan)}>Delete</button>
                        </div>
                    {/if}
                </li>
            {/each}
        </ul>
    {/if}
</section>

<style>
    .library { position: absolute; top: 0; right: 0; bottom: 0; z-index: 40; width: min(440px, 100%); padding: 20px; overflow-y: auto; background: var(--panel); box-shadow: -8px 0 24px rgb(0 0 0 / .15); color: var(--ink); font: 14px var(--sans); }
    header { display: flex; align-items: center; justify-content: space-between; gap: 16px; }
    h2 { margin: 0; font: 700 22px var(--sans); }
    .storage, .empty { color: var(--ink-soft); line-height: 1.5; }
    .file { display: none; }
    .error { color: var(--coral); line-height: 1.5; }
    ul { margin: 24px 0 0; padding: 0; list-style: none; }
    li { padding: 16px 0; border-top: 1px solid var(--line); }
    .open { display: grid; gap: 6px; width: 100%; padding: 8px; border: 0; border-radius: 6px; background: transparent; color: var(--ink); font: inherit; text-align: left; cursor: pointer; }
    .open:hover, .open[aria-current="true"] { background: var(--parchment-2); }
    strong { overflow-wrap: anywhere; font-size: 16px; }
    span, small { color: var(--ink-soft); }
    small { font-size: 12px; font-variant-numeric: tabular-nums; }
    .actions { display: flex; flex-wrap: wrap; gap: 4px; margin-top: 8px; }
    form { display: flex; flex-wrap: wrap; gap: 8px; }
    label { display: grid; gap: 6px; width: 100%; }
    input { width: 100%; padding: 8px; border: 1px solid var(--line); border-radius: 6px; background: var(--parchment-2); color: var(--ink); font: inherit; caret-color: var(--forest); }
    button:focus-visible, input:focus-visible { outline: 2px solid var(--forest); outline-offset: 2px; }
    button:disabled { opacity: .5; cursor: default; }
    .icon { display: grid; place-items: center; min-width: 36px; min-height: 36px; border: 0; background: transparent; color: var(--ink); cursor: pointer; }
</style>
