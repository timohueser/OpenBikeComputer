<script lang="ts">
    import { tick } from 'svelte';
    import Icon from './PlannerIcon.svelte';
    import { deleteVersion, listVersions, readVersion, saveVersion, type Version } from '../../lib/planner/versions';
    import type { Trip } from '../../lib/planner/editor';

    let { trip, draftSavedAt, draftError, onRestore, onSaved }: {
        trip: Trip;
        draftSavedAt: number | null;
        draftError: string;
        onRestore: (trip: Trip) => void;
        onSaved: (version: Version) => void;
    } = $props();

    let open = $state(false);
    let naming = $state(false);
    let name = $state('');
    let versions = $state<Version[]>([]);
    let root: HTMLDivElement;
    let menu = $state<HTMLDivElement>();
    let now = $state(Date.now());

    const draft = $derived(draftError || (draftSavedAt === null ? 'Draft · no changes this session' : `Draft · saved ${ago(draftSavedAt)}`));

    function ago(time: number) {
        const minutes = Math.floor((now - time) / 60000);
        return minutes < 1 ? 'just now' : minutes < 60 ? `${minutes} min ago` : `at ${when(new Date(time).toISOString())}`;
    }

    function when(iso: string) {
        const at = new Date(iso);
        const days = Math.round((new Date(now).setHours(0, 0, 0, 0) - new Date(iso).setHours(0, 0, 0, 0)) / 86400000);
        const day = days === 0 ? 'Today' : days === 1 ? 'Yesterday' : at.toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
        return `${day} ${at.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })}`;
    }

    function save(label?: string) {
        try {
            onSaved(saveVersion($state.snapshot(trip), label));
        } catch {
            return;
        }
        versions = listVersions();
        naming = false;
        name = '';
    }

    async function toggle() {
        open = !open;
        naming = false;
        if (!open) return;
        now = Date.now();
        versions = listVersions();
        await tick();
        menu?.querySelector<HTMLElement>('button')?.focus();
    }

    function restore(id: string) {
        const saved = readVersion(id);
        if (!saved) return;
        open = false;
        onRestore(saved);
    }

    function remove(id: string) {
        deleteVersion(id);
        versions = listVersions();
    }

    function outside(event: PointerEvent) {
        if (open && !root.contains(event.target as Node)) open = false;
    }

    function key(event: KeyboardEvent) {
        if (event.key !== 'Escape' || !open) return;
        event.stopPropagation();
        open = false;
        root.querySelector<HTMLElement>('.more')?.focus();
    }
</script>

<svelte:window onpointerdown={outside} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="versions" bind:this={root} onkeydown={key}>
    <div class="split">
        <button type="button" class="save" onclick={() => save()}>Save</button>
        <button type="button" class="more" aria-label="Saved versions" aria-expanded={open} aria-haspopup="true" onclick={toggle}><Icon name="down" size={14} /></button>
    </div>
    {#if open}
        <div class="menu" bind:this={menu}>
            <p class="draft">{draft}</p>
            {#if versions.length}
                <ul>
                    {#each versions as version (version.id)}
                        <li>
                            <span class="what">
                                <strong>{version.name ?? when(version.at)}</strong>
                                <small>{version.name ? `${when(version.at)} · ` : ''}{version.summary}</small>
                            </span>
                            <button type="button" class="link" onclick={() => restore(version.id)}>Restore</button>
                            <button type="button" class="delete" aria-label={`Delete ${version.name ?? when(version.at)}`} onclick={() => remove(version.id)}><Icon name="close" size={14} /></button>
                        </li>
                    {/each}
                </ul>
            {:else}
                <p class="empty">No saved versions yet. Save keeps a copy you can return to.</p>
            {/if}
            {#if naming}
                <form class="name" onsubmit={(event) => { event.preventDefault(); if (name.trim()) save(name); }}>
                    <!-- svelte-ignore a11y_autofocus -->
                    <input aria-label="Version name" placeholder="Version name" maxlength="60" bind:value={name} autofocus />
                    <button type="submit" class="outline" disabled={!name.trim()}>Save</button>
                </form>
            {:else}
                <button type="button" class="name-start" onclick={() => naming = true}>Name this version…</button>
            {/if}
        </div>
    {/if}
</div>

<style>
    .versions {
        position: relative;
    }
    .split {
        display: flex;
        height: 32px;
        border: 1px solid var(--ink);
        border-radius: 6px;
        overflow: hidden;
    }
    .split button {
        border: 0;
        background: transparent;
        color: var(--ink);
        font: 600 13px var(--sans);
        cursor: pointer;
    }
    .split button:hover {
        background: var(--parchment-2);
    }
    .save {
        padding: 0 14px;
    }
    .more {
        display: grid;
        place-items: center;
        width: 30px;
    }
    .split button + button {
        border-left: 1px solid var(--ink);
    }
    .menu {
        position: absolute;
        top: 40px;
        right: 0;
        z-index: 30;
        width: 340px;
        padding: 8px;
        border-radius: 8px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
    }
    .draft,
    .empty {
        margin: 0;
        padding: 8px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    ul {
        margin: 0 0 4px;
        padding: 4px 0;
        list-style: none;
        border-block: 1px solid var(--line);
        max-height: 300px;
        overflow: auto;
    }
    li {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 8px;
        border-radius: 6px;
    }
    li:hover {
        background: var(--parchment-2);
    }
    .what {
        flex: 1;
        min-width: 0;
    }
    .what strong,
    .what small {
        display: block;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .what strong {
        font: 600 14px var(--sans);
        font-variant-numeric: tabular-nums;
    }
    .what small {
        margin-top: 2px;
        font-size: 13px;
        color: var(--ink-soft);
        font-variant-numeric: tabular-nums;
    }
    button {
        font: inherit;
        color: inherit;
        cursor: pointer;
    }
    .link {
        border: 0;
        padding: 4px 0;
        background: none;
        font-size: 13px;
        font-weight: 600;
        color: var(--forest);
        text-decoration: underline;
        text-underline-offset: 3px;
    }
    .delete {
        display: grid;
        place-items: center;
        width: 24px;
        height: 24px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-faint);
    }
    .delete:hover {
        color: var(--ink);
    }
    .name-start {
        width: 100%;
        padding: 8px;
        border: 0;
        border-radius: 6px;
        background: none;
        text-align: left;
        font-size: 13px;
        font-weight: 600;
    }
    .name-start:hover {
        background: var(--parchment-2);
    }
    .name {
        display: flex;
        gap: 8px;
        padding: 8px;
    }
    .name input {
        flex: 1;
        min-width: 0;
        height: 32px;
        padding: 0 10px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font-size: 13px;
    }
    .outline {
        height: 32px;
        padding: 0 12px;
        border: 1px solid var(--ink);
        border-radius: 6px;
        background: none;
        font-size: 13px;
        font-weight: 600;
    }
</style>
