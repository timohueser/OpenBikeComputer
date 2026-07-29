<!--
  Several GPX files landed at once: convert them all first, show what they are, and ask the one
  question worth asking — separate routes, or the stages of a named trip?

  Conversion runs up front and per file (`prepareRoute`, wasm, no device involved), so what the
  list shows is what the device will hold, and a file that fails to convert is named with the
  converter's own sentence while the rest stay usable. The order shown is the order uploaded and
  the trip's stage order; it starts filename-sorted (day 1 before day 10) and the arrows fix
  whatever that guessed wrong.

  A plain overlay, not a `<dialog>` — same WebKitGTK reasoning as `ConfirmDialog.svelte`.
-->
<script lang="ts">
    import { onMount } from "svelte";
    import { commonPrefixName, sortForTrip } from "../../lib/device/multidrop";
    import { prepareRoute, type PreparedRoute } from "../../lib/device/route";
    import { RETENTION_LEVELS, retentionLabel } from "../../lib/device/retention";
    import { formatBytes } from "../../lib/format";

    let {
        files,
        onadd,
        oncancel,
    }: {
        files: File[];
        /** The rider decided: upload these, in this order, grouped iff `tripName` is non-null,
         *  every stage stamped with the §4.4 cmd 6 `retention` level (0 = forever = stamp
         *  nothing). */
        onadd: (routes: PreparedRoute[], tripName: string | null, retention: number) => void;
        oncancel: () => void;
    } = $props();

    interface Row {
        readonly file: File;
        prepared: PreparedRoute | null;
        error: string | null;
    }

    // Deliberately captured once: the dialog is mounted fresh per drop (`{#if tripDrop}`), so the
    // list it works on is the drop it was opened for — a later prop change has nothing to mean.
    // svelte-ignore state_referenced_locally
    let rows = $state<Row[]>(sortForTrip(files).map((file) => ({ file, prepared: null, error: null })));
    // svelte-ignore state_referenced_locally
    let tripName = $state(commonPrefixName(files.map((f) => f.name)));
    /** Applied to every uploaded stage after its commit — same choice the drop tile offers. */
    let retention = $state(0);
    let converting = $state(true);

    onMount(() => {
        void (async () => {
            // Sequential rather than parallel: each conversion holds the wasm module briefly and
            // the list fills top-to-bottom, which reads as progress without a progress bar.
            for (const row of rows) {
                try {
                    row.prepared = await prepareRoute(row.file);
                } catch (cause) {
                    row.error = cause instanceof Error ? cause.message : String(cause);
                }
            }
            converting = false;
        })();
    });

    const good = $derived(rows.filter((row) => row.prepared !== null));
    const failed = $derived(rows.filter((row) => row.error !== null));

    function move(index: number, delta: number) {
        const to = index + delta;
        if (to < 0 || to >= rows.length) return;
        const next = [...rows];
        const [row] = next.splice(index, 1);
        next.splice(to, 0, row);
        rows = next;
    }

    function drop(index: number) {
        rows = rows.filter((_, i) => i !== index);
    }

    function summary(prepared: PreparedRoute): string {
        return [
            `${(prepared.header.distanceM / 1000).toFixed(1)} km`,
            `${prepared.header.ascentM} m up`,
            `${prepared.header.pointCount.toLocaleString()} points`,
            formatBytes(prepared.obcr.length),
        ].join(" · ");
    }

    function add(asTrip: boolean) {
        const routes = rows.map((row) => row.prepared).filter((p): p is PreparedRoute => p !== null);
        if (routes.length === 0) return;
        onadd(routes, asTrip ? tripName.trim() || "Trip" : null, retention);
    }

    function onKeydown(event: KeyboardEvent) {
        if (event.key === "Escape") {
            event.preventDefault();
            oncancel();
        }
    }
</script>

<svelte:window on:keydown={onKeydown} />

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && oncancel()}>
    <div class="sheet card" role="dialog" aria-modal="true" aria-labelledby="tripdrop-title">
        <h3 id="tripdrop-title">
            {rows.length}
            GPX files → {good.length || rows.length}
            {(good.length || rows.length) === 1 ? "route" : "routes"}
        </h3>

        <ul class="rows">
            {#each rows as row, index (row.file)}
                <li class:bad={row.error !== null}>
                    <span class="order">
                        <button type="button" class="nudge" disabled={index === 0} aria-label="Move up"
                            onclick={() => move(index, -1)}>↑</button>
                        <button type="button" class="nudge" disabled={index === rows.length - 1}
                            aria-label="Move down" onclick={() => move(index, 1)}>↓</button>
                    </span>
                    <span class="grow">
                        {#if row.prepared}
                            <p class="name">{row.prepared.header.name}</p>
                            <p class="small faint">{row.file.name} → {summary(row.prepared)}</p>
                        {:else if row.error}
                            <p class="name">{row.file.name}</p>
                            <p class="small error">{row.error}</p>
                        {:else}
                            <p class="name faint">{row.file.name}</p>
                            <p class="small faint">converting…</p>
                        {/if}
                    </span>
                    <button type="button" class="btn ghost" title="Leave this file out" aria-label="Leave out"
                        onclick={() => drop(index)}>×</button>
                </li>
            {/each}
        </ul>

        {#if failed.length > 0}
            <p class="small faint">
                {failed.length}
                {failed.length === 1 ? "file" : "files"} could not be converted and will be left out.
            </p>
        {/if}

        <div class="tripline">
            <label class="small soft" for="tripdrop-name">Group them as a trip?</label>
            <input
                id="tripdrop-name"
                type="text"
                placeholder="Trip name"
                bind:value={tripName}
                maxlength="48"
            />
            <label class="small soft keep" for="tripdrop-keep">
                keep on device:
                <select id="tripdrop-keep" bind:value={retention}>
                    {#each RETENTION_LEVELS as level (level)}
                        <option value={level}>{retentionLabel(level)}</option>
                    {/each}
                </select>
            </label>
        </div>

        <div class="actions">
            <button type="button" class="btn ghost" onclick={oncancel}>Cancel</button>
            <span class="right">
                <button type="button" class="btn" disabled={converting || good.length === 0}
                    onclick={() => add(false)}>
                    Add as separate routes
                </button>
                <button type="button" class="btn primary" disabled={converting || good.length === 0}
                    onclick={() => add(true)}>
                    Add as a trip
                </button>
            </span>
        </div>
    </div>
</div>

<style>
    .backdrop {
        position: fixed;
        inset: 0;
        z-index: 1500;
        background: rgba(32, 48, 29, 0.38);
        display: flex;
        align-items: center;
        justify-content: center;
        padding: 20px;
    }

    .sheet {
        width: min(600px, 100%);
        max-height: min(88vh, 620px);
        overflow-y: auto;
        display: flex;
        flex-direction: column;
        gap: 12px;
        box-shadow: 0 18px 44px rgba(32, 48, 29, 0.28);
    }

    h3 {
        font-family: var(--serif);
        font-size: 17px;
        margin: 0;
    }

    .rows {
        list-style: none;
        margin: 0;
        padding: 0;
    }

    .rows li {
        display: flex;
        align-items: center;
        gap: 10px;
        padding: 8px 0;
    }

    .rows li + li {
        border-top: 1px solid var(--line);
    }

    .rows li.bad {
        opacity: 0.75;
    }

    .rows p {
        margin: 0;
    }

    .grow {
        flex: 1;
        min-width: 0;
    }

    .name {
        font-family: var(--serif);
        font-size: 14.5px;
    }

    .name.faint {
        color: var(--ink-faint);
    }

    .order {
        display: flex;
        flex-direction: column;
        gap: 1px;
        flex: none;
    }

    .nudge {
        border: 0;
        background: none;
        padding: 0 3px;
        font-size: 11px;
        line-height: 1.2;
        color: var(--ink-faint);
        cursor: pointer;
    }

    .nudge:hover:not(:disabled) {
        color: var(--ink);
    }

    .nudge:disabled {
        opacity: 0.3;
        cursor: default;
    }

    .error {
        color: var(--coral);
    }

    .tripline {
        display: flex;
        align-items: center;
        gap: 10px;
        border-top: 1px solid var(--line);
        padding-top: 10px;
    }

    .tripline {
        flex-wrap: wrap;
    }

    .keep {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        margin-left: auto;
    }

    .keep select {
        font-size: 12.5px;
        padding: 3px 6px;
        border-radius: 7px;
    }

    .tripline input {
        flex: 1;
        min-width: 0;
        max-width: 280px;
        font: inherit;
        color: var(--ink);
        background: var(--panel);
        border: 1px solid var(--line-strong);
        border-radius: 8px;
        padding: 5px 10px;
    }

    .actions {
        display: flex;
        align-items: center;
        gap: 8px;
    }

    .actions .right {
        margin-left: auto;
        display: flex;
        gap: 8px;
    }
</style>
