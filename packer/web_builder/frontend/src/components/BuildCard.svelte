<script lang="ts">
    import { onMount, untrack } from "svelte";
    import { formatBytes } from "../lib/format";
    import { platform, type StartBuild } from "../lib/platform";
    import { buildConfigForSubmit, type SchemaEnvelope } from "../lib/config/model";
    import { working } from "../lib/config/storage.svelte";
    import { buildRegionIds, selectionReady, type AreaSelection } from "../lib/map/selection";

    // The build session comes in as a prop rather than off `platform` so this
    // card can only be mounted where `caps.build` holds: the host that can't
    // build has `buildMap === null` and there is nothing to pass.
    let {
        selection,
        buildMap,
    }: { selection: AreaSelection | null; buildMap: StartBuild } = $props();

    let filename = $state("mymap.obcm");
    let schema = $state<SchemaEnvelope | null>(null);
    let schemaNote = $state<string | null>(null);
    let strippedNote = $state<string | null>(null);
    let validation = $state<string | null>(null);
    // One session per mount, deliberately: a build in flight must not be
    // replaced because a prop identity changed underneath it.
    const tracker = untrack(() => buildMap());
    // Non-null on any host that can build (`schema` is gated on
    // `caps.build || caps.styleEditor`), which is the only place this mounts.
    const loadSchema = platform.schema;

    onMount(async () => {
        tracker.reattach();
        if (!loadSchema) return;
        try {
            schema = await loadSchema();
        } catch (e) {
            // A 503 means obc-pack isn't built — builds will fail, so surface
            // the server's build instructions up front.
            schema = null;
            schemaNote = e instanceof Error ? e.message : String(e);
        }
    });

    const busy = $derived(tracker.state === "starting" || tracker.state === "running");
    const ready = $derived(selection !== null && selectionReady(selection) && working.envelope !== null);

    // Both non-null only where the host has a filesystem: the desktop app writes
    // the map into a folder the user already has, so the "download" is a place
    // rather than a transfer. Read once — neither changes while the app runs.
    const revealFile = platform.revealFile;
    // Null on a host that cannot stop a build (see `BuildSession.cancel`), which
    // is why this is a member check and not a platform-name check.
    const stop = tracker.cancel;
    let revealError = $state<string | null>(null);

    async function reveal(path: string) {
        revealError = null;
        try {
            await revealFile?.(path);
        } catch (e) {
            revealError = e instanceof Error ? e.message : String(e);
        }
    }

    async function build() {
        validation = null;
        strippedNote = null;
        if (!selection || !selectionReady(selection)) {
            validation =
                selection?.mode === "bbox"
                    ? "Draw a box over land first — no downloadable region covers the current area."
                    : "Pick at least one region on the map first.";
            return;
        }
        if (!working.envelope) {
            validation = "Pick a map style first.";
            return;
        }
        const { config, strippedKeys } = buildConfigForSubmit(
            working.envelope.config,
            working.envelope.disabled,
            schema,
        );
        if (strippedKeys.length) {
            strippedNote = `Skipped settings this obc-pack build doesn't know yet: ${strippedKeys.join(", ")}.`;
        }
        await tracker.start({
            regionIds: buildRegionIds(selection),
            config,
            chunkSize: config.chunk_size ?? 4096,
            outputName: filename.trim() || "mymap.obcm",
            ...(selection.mode === "bbox" && selection.bbox ? { bbox: selection.bbox } : {}),
        });
    }
</script>

<div class="row">
    <input type="text" class="mono" bind:value={filename} aria-label="Output file name" />
    <button type="button" class="btn primary" onclick={build} disabled={busy || !ready}>
        {busy ? "Building…" : "Build map"}
    </button>
    {#if stop && busy}
        <button type="button" class="btn" onclick={stop}>Cancel</button>
    {/if}
</div>

{#if schemaNote}
    <p class="note error small">{schemaNote}</p>
{/if}
{#if validation}
    <p class="note error small">{validation}</p>
{/if}
{#if strippedNote}
    <p class="note small muted">{strippedNote}</p>
{/if}

{#if tracker.state !== "idle"}
    <div class="progress">
        <div class="bar">
            <div class="fill" style:width="{tracker.pct}%"></div>
        </div>
        <div class="labels small">
            <span class="muted">{tracker.phase || tracker.state}</span>
            <span class="faint">{tracker.pct}%</span>
        </div>
    </div>
{/if}

{#if tracker.state === "done" && tracker.result}
    <div class="done">
        {#if revealFile && tracker.result.path}
            <!-- A real filesystem: the map is already where it belongs, so the
                 action is "show me", not "download". -->
            {@const path = tracker.result.path}
            <button type="button" class="btn primary" onclick={() => reveal(path)}>
                Show {tracker.result.filename}
            </button>
            <span class="small muted">
                {formatBytes(tracker.result.size)} — copy it onto the device's SD card
            </span>
            <span class="small faint mono path">{path}</span>
        {:else}
            <a class="btn primary" href={tracker.result.downloadUrl} download={tracker.result.filename}>
                Download {tracker.result.filename}
            </a>
            <span class="small muted">{formatBytes(tracker.result.size)} — copy it onto the device's SD card</span>
        {/if}
    </div>
{/if}

{#if revealError}
    <p class="note error small">{revealError}</p>
{/if}

{#if tracker.state === "cancelled"}
    <!-- Muted, not red: this is what the user asked for. -->
    <p class="note small muted">Build cancelled — nothing was written.</p>
{/if}

{#if tracker.state === "error"}
    <p class="note error small">{tracker.error}</p>
{/if}

{#if tracker.logLines.length || tracker.transientLine}
    <details class="log">
        <summary class="small muted">Build log</summary>
        <pre class="mono small">{tracker.logLines.join("\n")}{tracker.transientLine
                ? "\n" + tracker.transientLine
                : ""}</pre>
    </details>
{/if}

<style>
    .row {
        display: flex;
        gap: 8px;
    }

    .row input {
        flex: 1;
        min-width: 0;
    }

    .note {
        margin: 8px 0 0;
    }

    .error {
        color: var(--coral);
    }

    .progress {
        margin-top: 12px;
    }

    .bar {
        height: 7px;
        background: var(--parchment-3);
        border-radius: 999px;
        overflow: hidden;
    }

    .fill {
        height: 100%;
        background: var(--forest);
        border-radius: 999px;
        transition: width 0.25s;
    }

    .labels {
        display: flex;
        justify-content: space-between;
        margin-top: 4px;
    }

    .done {
        margin-top: 12px;
        display: flex;
        align-items: center;
        gap: 10px;
        flex-wrap: wrap;
    }

    /* The full path, on its own line: it is the answer to "where did it go",
       and it is long. */
    .done .path {
        flex-basis: 100%;
        word-break: break-all;
    }

    .log {
        margin-top: 10px;
    }

    .log pre {
        background: #20301d;
        color: var(--parchment);
        border-radius: 10px;
        padding: 10px 12px;
        max-height: 220px;
        overflow: auto;
        white-space: pre-wrap;
        word-break: break-all;
        margin: 6px 0 0;
    }
</style>
