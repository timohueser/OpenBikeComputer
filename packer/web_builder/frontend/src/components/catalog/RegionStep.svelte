<script lang="ts">
    // Step 1 on a tier that downloads pre-baked maps: what the picked region
    // is, and — when it has no map of its own — where else to go.
    //
    // Regions nest, and OBCC §3 forbids treating a parent's artifact as a
    // child's or the other way round. So this never silently substitutes: it
    // *names* the containing region that is baked, or the sub-regions that are,
    // with their sizes, and the rider picks. That is also what keeps the
    // acceptance promise that an unbaked region is never a dead click — every
    // one of them leads either to a named neighbour in the catalog or to the
    // desktop app that can build it.

    import { DESKTOP_ROUTE } from "../../lib/routes";
    import { formatBytes } from "../../lib/format";
    import type { DeviceMapSupport } from "../../lib/catalog/availability";
    import { regionState } from "../../lib/catalog/availability";
    import type { CatalogArtifact } from "../../lib/catalog/manifest";
    import type { CatalogIndex, RegionEntry } from "../../lib/catalog/regions";

    let {
        index,
        entry,
        artifact,
        device,
        onselect,
    }: {
        index: CatalogIndex;
        entry: RegionEntry | null;
        /** The artifact for the picked region *and* the picked preset, if any. */
        artifact: CatalogArtifact | null;
        device: DeviceMapSupport | null;
        onselect: (regionId: string) => void;
    } = $props();

    const availability = $derived(entry ? regionState(entry, device) : null);
    const covering = $derived(entry ? index.ancestorsWithArtifacts(entry.id) : []);
    const inside = $derived(entry ? index.descendantsWithArtifacts(entry.id) : []);

    /** The smallest artifact of a region — what "how big is this" means when no
     *  preset is picked yet. */
    function smallest(of: RegionEntry): CatalogArtifact {
        return of.artifacts.reduce((a, b) => (b.bytes < a.bytes ? b : a));
    }
</script>

{#if !entry}
    <p class="summary muted small">
        Click a region on the map or search by name. Shaded regions have a map ready to download.
    </p>
{:else}
    <div class="head">
        <span class="name">{entry.name}</span>
        <span class="mono faint small">{entry.path}</span>
    </div>

    {#if availability === "available"}
        <p class="summary small">
            Whole region only — this download covers all of {entry.name}{artifact
                ? ` (${formatBytes(artifact.bytes)})`
                : ""}. Cropping to a smaller area needs
            <a href={DESKTOP_ROUTE}>the desktop app</a>.
        </p>
    {:else if availability === "unsupported"}
        <p class="summary warn small">
            Built as OBCM v{entry.artifacts[0].obcm_version}; the connected device reads v{device?.obcmVersion}.
            It can't open this map — update the device firmware, then pick it again.
        </p>
    {:else}
        <p class="summary small">
            No baked map for this region. <a href={DESKTOP_ROUTE}>The desktop app</a> builds one from
            the same OSM extract.
        </p>
        {#if covering.length}
            <div class="alts">
                <span class="small faint">Covered by</span>
                {#each covering as alt (alt.id)}
                    <button type="button" class="chip" onclick={() => onselect(alt.id)}>
                        {alt.name}
                        <span class="faint">from {formatBytes(smallest(alt).bytes)}</span>
                    </button>
                {/each}
            </div>
        {/if}
        {#if inside.length}
            <div class="alts">
                <span class="small faint">Baked inside it</span>
                {#each inside.slice(0, 8) as alt (alt.id)}
                    <button type="button" class="chip" onclick={() => onselect(alt.id)}>
                        {alt.name}
                        <span class="faint">from {formatBytes(smallest(alt).bytes)}</span>
                    </button>
                {/each}
                {#if inside.length > 8}
                    <span class="small faint">+{inside.length - 8} more</span>
                {/if}
            </div>
        {/if}
    {/if}
{/if}

<style>
    .head {
        display: flex;
        align-items: baseline;
        gap: 9px;
        flex-wrap: wrap;
        margin-bottom: 6px;
    }

    .name {
        font-weight: 600;
        font-size: 15px;
    }

    .summary {
        margin: 0;
        line-height: 1.45;
    }

    .summary.warn {
        color: var(--coral);
    }

    .alts {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 6px;
        margin-top: 8px;
    }

    .alts button.chip {
        gap: 7px;
        cursor: pointer;
    }

    .alts button.chip:hover {
        border-color: var(--wood);
    }
</style>
