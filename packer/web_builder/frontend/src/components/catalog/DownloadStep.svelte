<script lang="ts">
    // Step 3 on a tier that downloads pre-baked maps: the facts about the one
    // artifact that's been picked, and the download itself.
    //
    // The download is plain: fetch, verify against the manifest's `bytes` and
    // `sha256`, and only then hand the bytes to the browser (OBCC §7). A
    // mismatch surfaces here as an error and nothing is saved — the failure
    // mode this replaces is a corrupt map that only shows itself as a fault
    // screen on the device, hours away from a computer. Writing the same bytes
    // straight to a connected device is C4's (#903); this path is the one that
    // works with nothing attached and in any browser.

    import { untrack } from "svelte";
    import { artifactState, type DeviceMapSupport } from "../../lib/catalog/availability";
    import {
        artifactFilename,
        fetchArtifact,
        saveBytes,
        type DownloadProgress,
    } from "../../lib/catalog/download";
    import type { CatalogArtifact } from "../../lib/catalog/manifest";
    import type { RegionEntry } from "../../lib/catalog/regions";
    import type { Preset } from "../../lib/config/model";
    import { DESKTOP_ROUTE } from "../../lib/routes";
    import { formatBytes } from "../../lib/format";

    let {
        entry,
        preset,
        artifact,
        device,
    }: {
        entry: RegionEntry | null;
        preset: Preset | null;
        /** The picked (region, preset) pair, when the catalog has one. */
        artifact: CatalogArtifact | null;
        device: DeviceMapSupport | null;
    } = $props();

    type Phase = "idle" | "running" | "done" | "error";
    let phase = $state<Phase>("idle");
    let progress = $state<DownloadProgress | null>(null);
    let error = $state<string | null>(null);
    let savedAs = $state<string | null>(null);

    const support = $derived(artifact ? artifactState(artifact, device) : null);
    const canDownload = $derived(artifact !== null && support?.kind === "available");
    const pct = $derived(
        progress && progress.total > 0
            ? Math.min(100, Math.round((progress.received / progress.total) * 100))
            : 0,
    );

    // A new pick abandons the previous one: an in-flight fetch is aborted and
    // its outcome cleared, so "Saved …" can never sit under a different region
    // than the one it belongs to. `untrack` because this writes the same state
    // it would otherwise depend on.
    let running: AbortController | null = null;
    $effect(() => {
        void artifact;
        untrack(() => {
            running?.abort();
            running = null;
            phase = "idle";
            error = null;
            savedAs = null;
            progress = null;
        });
    });

    async function download() {
        if (!artifact || phase === "running") return;
        const run = new AbortController();
        running = run;
        phase = "running";
        error = null;
        savedAs = null;
        progress = { received: 0, total: artifact.bytes };
        try {
            const bytes = await fetchArtifact(artifact, {
                signal: run.signal,
                onProgress: (p) => {
                    if (running === run) progress = p;
                },
            });
            if (running !== run) return; // superseded by a new pick
            const filename = artifactFilename(artifact);
            saveBytes(bytes, filename);
            savedAs = filename;
            phase = "done";
        } catch (e) {
            if (running !== run) return;
            error = e instanceof Error ? e.message : String(e);
            phase = "error";
        }
    }
</script>

{#if !entry}
    <p class="line muted small">Pick a region to see what's available for it.</p>
{:else if !artifact}
    <p class="line small">
        {preset ? `${preset.name} isn't baked for ${entry.name}` : `Nothing baked for ${entry.name}`}.
        Pick another style, or build this region in <a href={DESKTOP_ROUTE}>the desktop app</a>.
    </p>
{:else if support?.kind === "unsupported"}
    <p class="line warn small">
        This map is OBCM v{support.artifactObcm} and the connected device reads v{support.deviceObcm}.
        Update the device firmware before downloading it.
    </p>
{:else}
    <div class="facts mono small">
        <span>{artifactFilename(artifact)}</span>
        <span class="faint">
            {formatBytes(artifact.bytes)} · built {artifact.built_at.slice(0, 10)} · OSM extract
            {artifact.source_snapshot} · OBCM v{artifact.obcm_version}
        </span>
    </div>

    <button
        type="button"
        class="btn primary"
        disabled={!canDownload || phase === "running"}
        onclick={download}
    >
        {phase === "running" ? `Downloading… ${pct}%` : "Download map"}
    </button>

    {#if phase === "running" && progress}
        <div class="bar"><span style:width={`${pct}%`}></span></div>
        <p class="line faint small">
            {formatBytes(progress.received)} of {formatBytes(progress.total)}
        </p>
    {/if}

    {#if phase === "done" && savedAs}
        <p class="line small">
            Saved {savedAs} — size and checksum match the catalog. Copy it to the top level of the
            device's card; the device loads the first <code>.obcm</code> it finds there.
        </p>
    {:else if phase === "error"}
        <p class="line warn small">Nothing was saved: {error}</p>
    {:else if phase === "idle"}
        <p class="line faint small">
            Verified against the catalog's checksum before it's saved.
        </p>
    {/if}
{/if}

<style>
    .line {
        margin: 8px 0 0;
        line-height: 1.45;
    }

    .line.warn {
        color: var(--coral);
    }

    .facts {
        display: flex;
        flex-direction: column;
        gap: 3px;
        margin-bottom: 10px;
        word-break: break-word;
    }

    .bar {
        margin-top: 9px;
        height: 6px;
        border-radius: 999px;
        background: var(--parchment-2);
        overflow: hidden;
    }

    .bar span {
        display: block;
        height: 100%;
        background: var(--forest);
        transition: width 0.2s;
    }
</style>
