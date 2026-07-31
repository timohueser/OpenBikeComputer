<script lang="ts">
    // The corridor panel (#1038) — mock R2·2, both states. Opened by the ◠
    // rail tool; everything in it is a *candidate* until "Add to map": checked
    // routes are pushed into the store's preview parts, the map draws them
    // dashed, the adds-line prices them live, and the one global width slider
    // (§8 U3's decided shape) re-buffers every checked route as it moves.
    //
    // The "From device" side ships as the decided affordance with an honest
    // stub behind it: §8 Q3 settled that connecting in step 1 is fine, but the
    // USB route listing itself is the send step's work (P4d). The tab exists,
    // the panel says exactly what is missing, and nothing pretends to connect.

    import { onDestroy, onMount } from "svelte";
    import { GpxError, parseGpx, type GpxRoute } from "../../lib/coverage/gpx";
    import { detailBandId, parseCells, patchCount } from "../../lib/coverage/shape";
    import {
        CORRIDOR_RADIUS_MAX_M,
        CORRIDOR_RADIUS_MIN_M,
        type CoverageStore,
    } from "../../lib/coverage/store.svelte";
    import { formatBytes } from "../../lib/format";
    import { confirmAction } from "../../lib/ui/confirm.svelte";

    let { store, onclose }: { store: CoverageStore; onclose: () => void } = $props();

    /**
     * Whether the panel may close (#1041 A7). The corridor tool is the one
     * tool holding user data the map cannot restore — files someone chose,
     * uploaded, maybe renamed their ride after — so Esc or a stray click on
     * another tool must not silently discard them. Committed parts are safe in
     * the selection; this guards only the panel's own uploaded rows, checked
     * or not.
     */
    export async function requestClose(): Promise<boolean> {
        if (routes.length === 0) return true;
        if (asking) return false; // one question at a time; a second Esc answered the first
        asking = true;
        try {
            const one = routes.length === 1;
            return await confirmAction({
                title: one
                    ? `Discard “${routes[0].route.name}”?`
                    : `Discard ${routes.length} uploaded routes?`,
                body: one
                    ? "It was uploaded but hasn't been added to the map."
                    : "They were uploaded but haven't been added to the map.",
                confirmLabel: "Discard",
                destructive: true,
            });
        } finally {
            asking = false;
        }
    }
    let asking = false;

    interface PanelRoute {
        id: string;
        route: GpxRoute;
        checked: boolean;
    }

    let routes = $state<PanelRoute[]>([]);
    let uploadError = $state<string | null>(null);
    let deviceNote = $state<string | null>(null);
    let source = $state<"gpx" | "device">("gpx");
    let dragOver = $state(false);
    let fileInput: HTMLInputElement | undefined = $state();
    let nextRouteId = 1;

    const detailBand = $derived(detailBandId(store.catalog));

    // Checked routes → the store's preview parts. This is the panel's one
    // side-effect, and it is kept in an effect so every path that changes a
    // checkbox, adds a file or removes a row goes through the same door.
    $effect(() => {
        store.previewParts = routes
            .filter((r) => r.checked)
            .map((r) => store.makePreviewPart(r.id, r.route.name, r.route.points));
    });

    // Closing the panel abandons the candidate — committed parts live in the
    // selection, not here.
    onDestroy(() => {
        store.previewParts = [];
    });

    // A dialog receives focus when it opens (#1041 low sweep): the keyboard
    // and a screen reader arrive where the conversation moved, instead of
    // being left on the rail button behind it. Non-modal on purpose — Tab
    // walks the panel's own controls and out again, like the app's other
    // dialog.
    let panelEl = $state<HTMLDivElement>();
    onMount(() => panelEl?.focus());

    async function addFiles(files: Iterable<File>) {
        uploadError = null;
        const problems: string[] = [];
        for (const file of files) {
            try {
                const route = parseGpx(await file.text(), file.name.replace(/\.gpx$/i, ""));
                routes.push({ id: `r${nextRouteId++}`, route, checked: true });
            } catch (e) {
                problems.push(`${file.name}: ${e instanceof GpxError ? e.message : "could not be read"}`);
            }
        }
        if (problems.length) uploadError = problems.join(" · ");
    }

    function onPick(e: Event) {
        const input = e.currentTarget as HTMLInputElement;
        if (input.files?.length) void addFiles(input.files);
        input.value = "";
    }

    function onDrop(e: DragEvent) {
        e.preventDefault();
        dragOver = false;
        const files = [...(e.dataTransfer?.files ?? [])].filter((f) => /\.gpx$/i.test(f.name));
        if (files.length) {
            // A drop is a GPX act wherever it lands: switch the panel to the
            // side that shows the rows it just gained, or they would arrive
            // invisibly behind the device stub (the two sources are exclusive
            // layouts, #1041 low sweep).
            source = "gpx";
            void addFiles(files);
        } else {
            uploadError = "Only .gpx files can become corridors.";
        }
    }

    function removeRoute(id: string) {
        routes = routes.filter((r) => r.id !== id);
    }

    // --- the adds-line ----------------------------------------------------

    const checkedCount = $derived(routes.filter((r) => r.checked).length);

    /** Disjoint patches the checked routes form at the detail band — the
     *  number behind "1 gap between routes". */
    const patches = $derived.by(() => {
        const previewed = store.previewResolution;
        if (!previewed) return 0;
        const cells = new Set<string>();
        for (const partRes of previewed.parts) {
            if (!partRes.part.id.startsWith("preview-")) continue;
            for (const id of partRes.cellsByBand.get(detailBand) ?? []) cells.add(id);
        }
        return cells.size ? patchCount(parseCells([...cells])) : 0;
    });

    const adds = $derived(store.previewSummary(patches));
    const previewError = $derived(store.previewed.error);

    const radiusKm = $derived(Math.round(store.selection.corridorRadiusM / 1000));

    function onRadius(e: Event) {
        store.setCorridorRadius(Number((e.currentTarget as HTMLInputElement).value) * 1000);
    }

    function commit() {
        store.commitPreview();
        routes = [];
        onclose();
    }
</script>

<div
    bind:this={panelEl}
    class="panel card"
    class:drag-over={dragOver}
    role="dialog"
    aria-label="Add corridor"
    tabindex="-1"
    ondragover={(e) => {
        e.preventDefault();
        dragOver = true;
    }}
    ondragleave={() => (dragOver = false)}
    ondrop={onDrop}
>
    <div class="head">
        <h4>Add corridor</h4>
        <!-- The mock's state-B tag was "web · no device yet"; the tier name is
             dropped because this panel also renders in the desktop app, where
             "web" would be a lie about where it is running (#1041 A12). -->
        <span class="small faint">no device yet</span>
        <button type="button" class="close" aria-label="Close the corridor panel" onclick={onclose}>✕</button>
    </div>

    <div class="sources">
        <button
            type="button"
            class:active={source === "gpx"}
            onclick={() => {
                source = "gpx";
                fileInput?.click();
            }}
        >
            ⤒ Upload GPX
        </button>
        <button type="button" class:active={source === "device"} onclick={() => (source = "device")}>
            From device
        </button>
        <input
            bind:this={fileInput}
            type="file"
            accept=".gpx,application/gpx+xml"
            multiple
            class="hidden-input"
            onchange={onPick}
        />
    </div>

    {#if source === "device"}
        <div class="device-stub">
            <!-- Future tense on purpose (#1041 A13): the mock's "Connect your
                 OBC to list the routes saved on it" was written for a working
                 flow, and above a stub it invited an action the button cannot
                 pay off. The mock's "one browser prompt" reassurance returns
                 with the real flow (P4d, recorded on the epic). -->
            <p class="small muted">The routes saved on your OBC will list here.</p>
            <!-- The affordance §8 Q3 decided (early connect is fine), with an
                 honest stub behind it: the USB route listing lands with the
                 send-to-device work, and this button says so rather than
                 opening a browser prompt it cannot yet pay off. -->
            <button
                type="button"
                class="btn primary connect"
                onclick={() =>
                    (deviceNote =
                        "Listing the routes saved on the device isn't wired up yet — it lands together " +
                        "with the send-to-device step. Upload a GPX of the same route meanwhile.")}
            >
                Connect device
            </button>
            {#if deviceNote}
                <p class="small stub-note">{deviceNote}</p>
            {/if}
        </div>
    {/if}

    {#if uploadError}
        <p class="small error">{uploadError}</p>
    {/if}

    {#if routes.length === 0 && source === "gpx"}
        <p class="small muted empty">
            Upload a route and the map around it — as wide as you choose, gaps and all — becomes part of
            this download. Or drop a .gpx file anywhere in this panel.
        </p>
    {/if}

    <!-- The two sources are exclusive layouts (#1041 low sweep): the uploaded
         rows, the width slider and the commit belong to the GPX side, and
         rendering them under the device stub conflated where the routes came
         from. Switching tabs keeps the rows — only the view changes. -->
    {#if routes.length && source === "gpx"}
        <ul class="routes">
            {#each routes as r (r.id)}
                <li>
                    <label>
                        <input type="checkbox" bind:checked={r.checked} />
                        <span class="name">{r.route.name}</span>
                    </label>
                    <span class="mono faint small">{Math.round(r.route.distanceKm)} km</span>
                    <button
                        type="button"
                        class="drop"
                        aria-label="Remove {r.route.name} from the panel"
                        onclick={() => removeRoute(r.id)}>✕</button
                    >
                </li>
            {/each}
        </ul>

        <div class="slider">
            <div class="slider-head">
                <!-- A real label, so the accessible name IS the visible text —
                     the old aria-label paraphrased it, which is the mismatch
                     WCAG 2.5.3 exists to forbid (#1041 low sweep). -->
                <label class="small muted" for="corridor-width-panel">Corridor width — all routes</label>
                <span class="mono small">± {radiusKm} km</span>
            </div>
            <input
                id="corridor-width-panel"
                type="range"
                min={CORRIDOR_RADIUS_MIN_M / 1000}
                max={CORRIDOR_RADIUS_MAX_M / 1000}
                step="1"
                value={radiusKm}
                oninput={onRadius}
            />
        </div>

        {#if previewError}
            <p class="small error">{previewError}</p>
        {:else if adds && checkedCount}
            <p class="mono small adds">
                adds {formatBytes(adds.addsBytes)} · {adds.addsCells}
                {adds.addsCells === 1 ? "cell" : "cells"}{adds.patches > 1
                    ? ` · ${adds.patches - 1} ${adds.patches === 2 ? "gap" : "gaps"} between routes`
                    : ""}
            </p>
        {:else if checkedCount}
            <p class="mono small adds faint">pricing…</p>
        {/if}

        <div class="commit">
            <button type="button" class="btn primary" disabled={checkedCount === 0} onclick={commit}>
                Add to map
            </button>
            <span class="small faint">gaps are fine — holes stay visible</span>
        </div>
    {/if}
</div>

<style>
    .panel {
        width: min(360px, 70vw);
        max-height: 100%;
        overflow-y: auto;
        display: flex;
        flex-direction: column;
        gap: 10px;
        box-shadow: 0 8px 22px rgba(36, 51, 28, 0.18);
    }

    .panel.drag-over {
        border-color: var(--forest);
        box-shadow: 0 0 0 3px rgba(60, 107, 57, 0.18);
    }

    .head {
        display: flex;
        align-items: baseline;
        gap: 10px;
    }

    .head h4 {
        font-family: var(--serif);
        font-size: 15.5px;
        margin: 0;
        flex: 1;
    }

    .close {
        background: none;
        border: none;
        color: var(--ink-faint);
        font-size: 14px;
        padding: 2px 4px;
    }

    .close:hover {
        color: var(--coral);
    }

    .sources {
        display: flex;
        gap: 8px;
    }

    .sources button {
        border: 1px solid var(--parchment-3);
        border-radius: 8px;
        background: var(--parchment);
        color: var(--ink-soft);
        padding: 6px 11px;
        font-size: 13px;
    }

    .sources button.active {
        border-color: var(--forest);
        color: var(--forest-deep);
    }

    .sources button:hover {
        border-color: var(--wood);
    }

    .hidden-input {
        display: none;
    }

    .device-stub {
        background: var(--parchment);
        border: 1px solid var(--line);
        border-radius: 9px;
        padding: 10px 12px;
        display: flex;
        flex-direction: column;
        gap: 6px;
    }

    .device-stub p {
        margin: 0;
    }

    .device-stub .connect {
        align-self: center;
    }

    .stub-note {
        color: var(--ink);
        background: rgba(227, 173, 51, 0.18);
        border-radius: 7px;
        padding: 6px 9px;
    }

    .empty {
        margin: 0;
        line-height: 1.45;
    }

    .error {
        margin: 0;
        color: var(--coral);
    }

    .routes {
        list-style: none;
        margin: 0;
        padding: 0;
        display: flex;
        flex-direction: column;
        gap: 5px;
    }

    .routes li {
        display: flex;
        align-items: center;
        gap: 8px;
        background: var(--parchment);
        border: 1px solid var(--line);
        border-radius: 8px;
        padding: 6px 10px;
    }

    .routes label {
        flex: 1;
        display: flex;
        align-items: center;
        gap: 8px;
        font-size: 13px;
        min-width: 0;
    }

    .routes .name {
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .routes .drop {
        background: none;
        border: none;
        color: var(--ink-faint);
        padding: 0 2px;
        font-size: 13px;
    }

    .routes .drop:hover {
        color: var(--coral);
    }

    .slider-head {
        display: flex;
        justify-content: space-between;
        margin-bottom: 4px;
    }

    .slider input[type="range"] {
        width: 100%;
        accent-color: var(--forest);
        padding: 0;
        border: none;
        background: none;
    }

    .adds {
        margin: 0;
    }

    .commit {
        display: flex;
        align-items: center;
        gap: 10px;
    }
</style>
