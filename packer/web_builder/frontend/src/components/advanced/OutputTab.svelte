<script lang="ts">
    import type { SchemaEnvelope } from "../../lib/config/model";
    import { working } from "../../lib/config/storage.svelte";
    import ColorControl from "./ColorControl.svelte";

    let { schema }: { schema: SchemaEnvelope | null } = $props();

    const env = $derived(working.envelope!);
    const chunkMax = $derived(
        (schema?.schema as { properties?: { chunk_size?: { maximum?: number } } } | undefined)
            ?.properties?.chunk_size?.maximum ?? 4106,
    );
</script>

<div class="card">
    <div class="row">
        <div>
            <h4>Position marker</h4>
            <p class="muted small">
                The arrow drawn at the GPS position. Its shape is fixed in firmware; pick a color
                that reads over both land and sea.
            </p>
        </div>
        <ColorControl
            value={env.config.marker.color}
            onchange={(v) => {
                env.config.marker.color = v;
                working.markModified();
            }}
        />
    </div>

    <div class="row">
        <div>
            <h4>Chunk size</h4>
            <p class="muted small">
                Quadtree chunk payload target in bytes — an expert knob. Larger chunks mean fewer
                index nodes; the maximum is the device reader's per-feature cap.
            </p>
        </div>
        <input
            type="number"
            min="256"
            max={chunkMax}
            step="256"
            value={env.config.chunk_size ?? 4096}
            oninput={(e) => {
                const v = parseInt(e.currentTarget.value, 10);
                if (Number.isFinite(v)) {
                    env.config.chunk_size = v;
                    working.markModified();
                }
            }}
        />
    </div>

    <div class="row">
        <div>
            <h4>Merge fills</h4>
            <p class="muted small">
                Dissolve fill polygons that render identically — same colour, no outline — into one
                shape per zoom level, dropping shared parcel boundaries. A pure size and render-cost
                win with no visual change; off packs exactly as before.
            </p>
        </div>
        <label class="toggle">
            <input
                type="checkbox"
                checked={env.config.merge_fills ?? false}
                onchange={(e) => {
                    env.config.merge_fills = e.currentTarget.checked;
                    working.markModified();
                }}
            />
        </label>
    </div>

    <div class="row">
        <div>
            <h4>Merge lines</h4>
            <p class="muted small">
                Stitch same-styled road/path/rail fragments — one OSM way split into many segments —
                into continuous polylines per zoom level. Reclaims the per-feature scratch that
                saturates at mid zoom; solid lines look identical, a dashed or cased line runs
                continuous across former joins. Off packs exactly as before.
            </p>
        </div>
        <label class="toggle">
            <input
                type="checkbox"
                checked={env.config.merge_lines ?? false}
                onchange={(e) => {
                    env.config.merge_lines = e.currentTarget.checked;
                    working.markModified();
                }}
            />
        </label>
    </div>

    <div class="row">
        <div>
            <h4>Packer</h4>
            <p class="muted small">
                The editor's capability follows the schema served by the obc-pack binary on this
                machine.
            </p>
        </div>
        <span class="small mono muted">
            {#if schema}
                OBCM v{schema.format_version ?? "?"} · schema {schema.schema_version} · {schema.source}
            {:else}
                schema unavailable
            {/if}
        </span>
    </div>
</div>

<style>
    .row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 24px;
        padding: 13px 0;
    }

    .row + .row {
        border-top: 1px solid var(--line);
    }

    h4 {
        margin: 0 0 3px;
        font-size: 14.5px;
        font-family: var(--sans);
        font-weight: 600;
    }

    p {
        margin: 0;
        max-width: 52ch;
    }

    input[type="number"] {
        width: 108px;
    }

    .toggle {
        display: inline-flex;
        align-items: center;
    }

    .toggle input {
        width: 20px;
        height: 20px;
        cursor: pointer;
        accent-color: var(--forest);
    }
</style>
