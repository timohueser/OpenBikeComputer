<script lang="ts">
    import { untrack } from "svelte";
    import type { SkinEntry, SkinStyle } from "../../lib/catalog/manifest";
    import type { CoverageStore } from "../../lib/coverage/store.svelte";
    import { cloneSkin, isCustomSkinId } from "../../lib/skin/custom";
    import OptionalSkinColor from "./OptionalSkinColor.svelte";
    import SkinColorControl from "./SkinColorControl.svelte";
    import SkinLivePreview from "./SkinLivePreview.svelte";

    let {
        store,
        base,
        onclose,
    }: {
        store: CoverageStore;
        base: SkinEntry;
        onclose: () => void;
    } = $props();

    const initial = untrack(() => base);
    let draft = $state(cloneSkin(initial));
    let name = $state(isCustomSkinId(initial.id) ? initial.name : `My ${initial.name}`);
    let query = $state("");
    let saveError = $state<string | null>(null);
    let nameInput = $state<HTMLInputElement>();

    interface StyleGroup {
        name: string;
        rows: Array<{ index: number; label: string; style: SkinStyle }>;
    }

    const groups = $derived.by<StyleGroup[]>(() => {
        const needle = query.trim().toLocaleLowerCase();
        const byName = new Map<string, StyleGroup>();
        draft.styles.forEach((style, index) => {
            const dot = style.feature_type.indexOf(".");
            const groupName = dot < 0 ? "other" : style.feature_type.slice(0, dot);
            const label = dot < 0 ? style.feature_type : style.feature_type.slice(dot + 1);
            if (needle && !style.feature_type.toLocaleLowerCase().includes(needle)) return;
            let group = byName.get(groupName);
            if (!group) {
                group = { name: groupName, rows: [] };
                byName.set(groupName, group);
            }
            group.rows.push({ index, label, style });
        });
        return [...byName.values()];
    });

    function integer(raw: string, min: number, max: number): number {
        const value = Number.parseInt(raw, 10);
        return Math.max(min, Math.min(max, Number.isFinite(value) ? value : 0));
    }

    function save() {
        saveError = null;
        try {
            store.saveCustomSkin(draft, name, base.id);
            onclose();
        } catch (cause) {
            saveError = cause instanceof Error ? cause.message : String(cause);
        }
    }

    function onKeydown(event: KeyboardEvent) {
        if (event.key === "Escape") {
            event.preventDefault();
            onclose();
        }
    }

    $effect(() => {
        nameInput?.focus();
    });
</script>

<svelte:window onkeydown={onKeydown} />

<div class="backdrop" role="presentation" onclick={(event) => event.target === event.currentTarget && onclose()}>
    <div class="sheet" role="dialog" aria-modal="true" aria-labelledby="skin-editor-title">
        <header>
            <div>
                <p class="eyebrow small faint">Skin editor</p>
                <h2 id="skin-editor-title">Make the map yours</h2>
                <p class="small faint intro">Colors, widths, dashes, drawing order and route marker only. The baked schema and LODs stay fixed.</p>
            </div>
            <button type="button" class="iconbtn" aria-label="Close the skin editor" onclick={onclose}>✕</button>
        </header>

        <div class="body">
            <aside>
                <div class="sticky">
                    <SkinLivePreview schemaJson={store.rootBody} skin={draft} />
                    <label class="name-field">
                        <span class="small">Skin name</span>
                        <input bind:this={nameInput} bind:value={name} maxlength="64" />
                    </label>
                    <div class="marker-row">
                        <span class="small">Route marker</span>
                        <SkinColorControl
                            value={draft.marker_color}
                            label="route marker"
                            onchange={(value) => (draft.marker_color = value)}
                        />
                    </div>
                </div>
            </aside>

            <main>
                <label class="search">
                    <span class="small">Find a feature</span>
                    <input type="search" bind:value={query} placeholder="road, water, forest…" />
                </label>

                <div class="table-head small faint" aria-hidden="true">
                    <span>Feature</span><span>color</span><span>color 2</span><span>width</span><span>order</span><span>dash</span>
                </div>
                {#each groups as group (group.name)}
                    <section class="group">
                        <h3>{group.name}</h3>
                        {#each group.rows as row (row.style.feature_type)}
                            <div class="style-row">
                                <span class="feature mono" title={row.style.feature_type}>{row.label}</span>
                                <SkinColorControl
                                    value={row.style.color}
                                    label={`${row.style.feature_type} color`}
                                    onchange={(value) => (draft.styles[row.index].color = value)}
                                />
                                <OptionalSkinColor
                                    value={row.style.color2}
                                    label={`${row.style.feature_type} second color`}
                                    onchange={(value) => (draft.styles[row.index].color2 = value)}
                                />
                                <input
                                    class="num"
                                    type="number"
                                    min="0"
                                    max="255"
                                    value={row.style.weight}
                                    aria-label={`${row.style.feature_type} width`}
                                    oninput={(event) =>
                                        (draft.styles[row.index].weight = integer(event.currentTarget.value, 0, 255))}
                                />
                                <input
                                    class="num"
                                    type="number"
                                    min="-128"
                                    max="127"
                                    value={row.style.z_index}
                                    aria-label={`${row.style.feature_type} z index`}
                                    oninput={(event) =>
                                        (draft.styles[row.index].z_index = integer(event.currentTarget.value, -128, 127))}
                                />
                                <input
                                    class="check"
                                    type="checkbox"
                                    checked={row.style.dashed}
                                    aria-label={`${row.style.feature_type} dashed`}
                                    onchange={(event) => (draft.styles[row.index].dashed = event.currentTarget.checked)}
                                />
                            </div>
                        {/each}
                    </section>
                {:else}
                    <p class="empty small faint">No feature matches “{query}”.</p>
                {/each}
            </main>
        </div>

        <footer>
            {#if saveError}<p class="save-error small" role="alert">{saveError}</p>{/if}
            <span class="spacer"></span>
            <button type="button" class="btn ghost" onclick={onclose}>Cancel</button>
            <button type="button" class="btn" onclick={() => (draft = cloneSkin(base))}>Reset</button>
            <button type="button" class="btn primary" disabled={!name.trim()} onclick={save}>Save custom skin</button>
        </footer>
    </div>
</div>

<style>
    .backdrop {
        position: fixed;
        inset: 0;
        z-index: 2200;
        display: grid;
        place-items: center;
        padding: 18px;
        background: rgba(27, 36, 23, 0.52);
        backdrop-filter: blur(3px);
    }

    .sheet {
        display: flex;
        flex-direction: column;
        width: min(1120px, 100%);
        max-height: min(900px, calc(100vh - 36px));
        overflow: hidden;
        color: var(--ink);
        background: var(--panel);
        border: 1px solid var(--parchment-3);
        border-radius: 18px;
        box-shadow: 0 24px 70px rgba(27, 36, 23, 0.35);
    }

    header,
    footer {
        display: flex;
        align-items: center;
        gap: 12px;
        padding: 15px 18px;
        border-color: var(--line);
    }

    header {
        justify-content: space-between;
        border-bottom: 1px solid var(--line);
    }

    header h2,
    header p {
        margin: 0;
    }

    .eyebrow {
        text-transform: uppercase;
        letter-spacing: 0.09em;
    }

    .intro {
        margin-top: 3px;
    }

    .body {
        display: grid;
        grid-template-columns: minmax(270px, 360px) minmax(520px, 1fr);
        min-height: 0;
        overflow: auto;
    }

    aside {
        padding: 18px;
        background: var(--parchment);
        border-right: 1px solid var(--line);
    }

    .sticky {
        position: sticky;
        top: 18px;
    }

    .name-field,
    .search {
        display: grid;
        gap: 5px;
    }

    .name-field {
        margin-top: 18px;
    }

    .name-field input,
    .search input {
        width: 100%;
        box-sizing: border-box;
    }

    .marker-row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        margin-top: 12px;
        padding-top: 12px;
        border-top: 1px solid var(--line);
    }

    main {
        min-width: 0;
        padding: 18px;
    }

    .search {
        margin-bottom: 16px;
    }

    .table-head,
    .style-row {
        display: grid;
        grid-template-columns: minmax(130px, 1fr) 38px 64px 56px 56px 36px;
        align-items: center;
        gap: 8px;
    }

    .table-head {
        padding: 0 8px 6px;
        text-align: center;
    }

    .table-head span:first-child {
        text-align: left;
    }

    .group {
        margin-bottom: 16px;
        overflow: hidden;
        border: 1px solid var(--line);
        border-radius: 12px;
    }

    .group h3 {
        margin: 0;
        padding: 7px 9px;
        font-size: 12px;
        letter-spacing: 0.08em;
        text-transform: uppercase;
        color: var(--ink-faint);
        background: var(--parchment-2);
        border-bottom: 1px solid var(--line);
    }

    .style-row {
        min-height: 38px;
        padding: 4px 8px;
        border-bottom: 1px solid var(--line);
    }

    .style-row:last-child {
        border-bottom: 0;
    }

    .feature {
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        font-size: 12px;
    }

    .num {
        min-width: 0;
        width: 100%;
        box-sizing: border-box;
        padding: 5px;
        text-align: center;
    }

    .check {
        justify-self: center;
        width: 17px;
        height: 17px;
    }

    .empty {
        padding: 30px;
        text-align: center;
    }

    footer {
        border-top: 1px solid var(--line);
    }

    .spacer {
        flex: 1;
    }

    .save-error {
        margin: 0;
        color: var(--coral);
    }

    @media (max-width: 840px) {
        .body {
            display: block;
        }

        aside {
            border-right: 0;
            border-bottom: 1px solid var(--line);
        }

        .sticky {
            position: static;
        }

        :global(.preview) {
            margin-inline: auto;
        }

        main {
            min-width: 720px;
        }

        footer {
            flex-wrap: wrap;
        }
    }
</style>
