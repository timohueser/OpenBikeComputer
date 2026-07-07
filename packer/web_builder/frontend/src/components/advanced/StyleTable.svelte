<script lang="ts">
    // One category's style rows: enable toggle, drag-reorder (order assigns
    // style IDs in the packer!), color, weight/z/priority, min-LOD segments,
    // plus schema-declared extra fields (v6 line_style/color2 land here
    // automatically). All edits mutate the working envelope + markModified.
    import { newStyleDef, removeCategory, reorderCategory } from "../../lib/config/edit";
    import { working } from "../../lib/config/storage.svelte";
    import ColorControl from "./ColorControl.svelte";
    import LodSegments from "./LodSegments.svelte";
    import SchemaField from "./SchemaField.svelte";

    let {
        cat,
        extras,
        catalogValues,
        ondeleted,
    }: {
        cat: string;
        extras: [string, Record<string, unknown>][];
        catalogValues: string[];
        ondeleted: () => void;
    } = $props();

    const env = $derived(working.envelope!);
    const entries = $derived(env.config.features[cat] ?? {});
    const lodCount = $derived(env.config.lods.length);

    let order = $state<string[]>([]);
    $effect(() => {
        order = Object.keys(entries);
    });

    // Drag-reorder: rows are draggable only while their handle is held, so
    // the inputs stay usable. The order array is the visual truth during the
    // drag; the features object is rebuilt key-by-key on drop.
    let armed = $state<string | null>(null);
    let dragging = $state<string | null>(null);

    function dragOver(e: DragEvent, name: string) {
        if (!dragging || dragging === name) return;
        e.preventDefault();
        const row = (e.currentTarget as HTMLElement).getBoundingClientRect();
        const before = e.clientY - row.top < row.height / 2;
        const next = order.filter((n) => n !== dragging);
        const idx = next.indexOf(name);
        next.splice(before ? idx : idx + 1, 0, dragging);
        order = next;
    }

    function dragEnd() {
        if (dragging) {
            reorderCategory(env.config, cat, order);
            working.markModified();
        }
        dragging = null;
        armed = null;
    }

    function isOn(name: string): boolean {
        return !env.disabled.includes(`${cat}/${name}`);
    }

    function setOn(name: string, on: boolean) {
        const key = `${cat}/${name}`;
        env.disabled = on ? env.disabled.filter((k) => k !== key) : [...env.disabled, key];
        working.markModified();
    }

    function removeType(name: string) {
        delete env.config.features[cat][name];
        env.disabled = env.disabled.filter((k) => k !== `${cat}/${name}`);
        working.markModified();
    }

    function deleteCategory() {
        const n = Object.keys(entries).length;
        if (!confirm(`Remove the "${cat}" category and all ${n} of its types?`)) return;
        const keys = removeCategory(env.config, cat);
        env.disabled = env.disabled.filter((k) => !keys.includes(k));
        working.markModified();
        ondeleted();
    }

    // Inline "add type" with the catalog's values for this key autocompleted.
    let adding = $state(false);
    let newName = $state("");
    let dupe = $state(false);

    function commitAdd() {
        const name = newName.trim();
        if (!name) {
            adding = false;
            return;
        }
        if (entries[name]) {
            dupe = true;
            return;
        }
        env.config.features[cat][name] = newStyleDef(env.config);
        working.markModified();
        adding = false;
        newName = "";
    }

    const gridCols = $derived(
        `20px 24px minmax(96px, 1fr) 84px ${extras.map(() => "92px").join(" ")} 48px 48px 46px max-content 24px`.replace(/\s+/g, " "),
    );

    function numEdit(name: string, field: "z_index" | "weight", raw: string) {
        const v = parseInt(raw, 10);
        entries[name][field] = Number.isFinite(v) ? v : 0;
        working.markModified();
    }
</script>

<div class="table card">
    <div class="head">
        <span class="small muted">{Object.keys(entries).length} types — drag ⋮⋮ to reorder (order sets draw priority on ties)</span>
        <button type="button" class="del-cat small" onclick={deleteCategory}>× remove category</button>
    </div>

    <div class="grid" style:grid-template-columns={gridCols} role="table">
        <span></span>
        <span class="h">on</span>
        <span class="h">type</span>
        <span class="h">color</span>
        {#each extras as [name] (name)}
            <span class="h mono">{name}</span>
        {/each}
        <span class="h" title="Stroke width in pixels (lines only)">w</span>
        <span class="h" title="Painter's order; lower is drawn first">z</span>
        <span class="h" title="Chunk-overflow drop order: 1 kept longest">prio</span>
        <span class="h" title="The coarsest LOD tier this feature appears from">levels</span>
        <span></span>

        {#each order as name (name)}
            {#if entries[name]}
                {@const def = entries[name]}
                <div
                    class="row"
                    class:off={!isOn(name)}
                    class:dragging={dragging === name}
                    style:grid-template-columns={gridCols}
                    role="row"
                    tabindex="-1"
                    draggable={armed === name}
                    ondragstart={(e) => {
                        dragging = name;
                        e.dataTransfer!.effectAllowed = "move";
                        e.dataTransfer!.setData("text/plain", name);
                    }}
                    ondragover={(e) => dragOver(e, name)}
                    ondragend={dragEnd}
                >
                    <button
                        type="button"
                        class="handle"
                        title="Drag to reorder"
                        aria-label="Drag to reorder {name}"
                        onmousedown={() => (armed = name)}
                        onmouseup={() => (armed = null)}>⋮⋮</button
                    >
                    <input
                        type="checkbox"
                        checked={isOn(name)}
                        title="Include in the build"
                        onchange={(e) => setOn(name, e.currentTarget.checked)}
                    />
                    <span class="mono name">{name}</span>
                    <ColorControl
                        value={def.color}
                        onchange={(v) => {
                            def.color = v;
                            working.markModified();
                        }}
                    />
                    {#each extras as [ename, eprop] (ename)}
                        <SchemaField
                            name={ename}
                            prop={eprop}
                            value={def[ename]}
                            onchange={(v) => {
                                // undefined ⇒ the field is cleared (optional
                                // color2): drop the key so it's absent from the
                                // emitted JSON, not present-but-undefined.
                                if (v === undefined) delete def[ename];
                                else def[ename] = v;
                                working.markModified();
                            }}
                        />
                    {/each}
                    <input
                        type="number"
                        class="num"
                        value={def.weight ?? 1}
                        min="0"
                        max="255"
                        oninput={(e) => numEdit(name, "weight", e.currentTarget.value)}
                    />
                    <input
                        type="number"
                        class="num"
                        value={def.z_index ?? 0}
                        min="-128"
                        max="127"
                        oninput={(e) => numEdit(name, "z_index", e.currentTarget.value)}
                    />
                    <select
                        value={String(def.priority ?? 3)}
                        onchange={(e) => {
                            def.priority = parseInt(e.currentTarget.value, 10);
                            working.markModified();
                        }}
                    >
                        {#each ["1", "2", "3", "4"] as p (p)}
                            <option value={p}>{p}</option>
                        {/each}
                    </select>
                    <LodSegments
                        count={lodCount}
                        value={def.min_lod ?? 0}
                        onchange={(v) => {
                            def.min_lod = v;
                            working.markModified();
                        }}
                    />
                    <button
                        type="button"
                        class="del"
                        title="Remove type"
                        aria-label="Remove {name}"
                        onclick={() => removeType(name)}>×</button
                    >
                </div>
            {/if}
        {/each}
    </div>

    {#if adding}
        <input
            type="text"
            class="add-input"
            class:dupe
            list="vals-{cat}"
            placeholder={`OSM ${cat} value (e.g. "steps")`}
            bind:value={newName}
            oninput={() => (dupe = false)}
            onkeydown={(e) => {
                if (e.key === "Enter") commitAdd();
                else if (e.key === "Escape") (adding = false), (newName = "");
            }}
            onblur={commitAdd}
        />
        <datalist id="vals-{cat}">
            {#each catalogValues.filter((v) => !entries[v]) as v (v)}
                <option value={v}></option>
            {/each}
        </datalist>
    {:else}
        <button type="button" class="add small" onclick={() => (adding = true)}>+ add type</button>
    {/if}
</div>

<style>
    .table {
        padding: 12px;
        min-width: 0;
        flex: 1;
    }

    .head {
        display: flex;
        justify-content: space-between;
        align-items: baseline;
        gap: 10px;
        margin-bottom: 8px;
    }

    .del-cat {
        background: none;
        border: none;
        color: var(--ink-faint);
    }

    .del-cat:hover {
        color: var(--coral);
    }

    .grid {
        display: grid;
        gap: 6px 8px;
        align-items: center;
        font-size: 13px;
    }

    .grid > .h {
        font-size: 11px;
        color: var(--ink-faint);
        text-transform: lowercase;
    }

    .row {
        display: grid;
        grid-column: 1 / -1;
        gap: 6px 8px;
        align-items: center;
        border-top: 1px solid var(--line);
        padding-top: 6px;
    }

    .row.off > :not(.handle):not(input[type="checkbox"]) {
        opacity: 0.4;
    }

    .row.dragging {
        opacity: 0.5;
    }

    .handle {
        background: none;
        border: none;
        color: var(--ink-faint);
        cursor: grab;
        padding: 0;
        font-size: 13px;
        letter-spacing: -2px;
    }

    .name {
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .num,
    select {
        padding: 3px 6px;
        font-size: 12.5px;
        border-radius: 6px;
        width: 100%;
    }

    .del {
        background: none;
        border: none;
        color: var(--ink-faint);
        font-size: 15px;
        padding: 0;
    }

    .del:hover {
        color: var(--coral);
    }

    .add {
        margin-top: 9px;
        background: none;
        border: none;
        color: var(--forest);
        padding: 0;
    }

    .add-input {
        margin-top: 9px;
        font-size: 12.5px;
        width: min(260px, 100%);
    }

    .add-input.dupe {
        border-color: var(--coral);
    }
</style>
