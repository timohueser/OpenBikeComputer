<script lang="ts">
    import type { Bbox } from "../../lib/data/areas";
    import { CONTENTS, layerFlags, needs, shows, TERRAIN, toggled, unshown } from "../../lib/data/contents";
    import { planArgs, type Flags } from "../../lib/data/flags";
    import type { BakeRoute } from "../../lib/data/screens";
    import type { Started } from "../../lib/data/shapes";
    import { prepareHash } from "../../lib/data/screens";
    import { list } from "../../lib/data/shapes";
    import { modelsOf, referenceOf, type SourceRow } from "../../lib/data/sources";
    import Action from "./Action.svelte";

    // What the bake makes: one card for each part. A card says what it needs in the area and which
    // card shows it; one that cannot bake here, or that nothing on shows, says why and cannot be on.
    let {
        to,
        area,
        bounds,
        countries,
        on,
        extra,
        sources,
        locked,
        change,
        planned,
    }: {
        to: string;
        area: NonNullable<BakeRoute["area"]>;
        bounds: Bbox | null;
        countries: string[];
        on: string[];
        /** Settings flags from a link of another screen, such as `--only`. */
        extra: Flags;
        /** `null` until `obc data sources` answers. */
        sources: SourceRow[] | null;
        locked: string | null;
        change: (on: string[]) => void;
        planned: (run: string) => void;
    } = $props();

    const cards = $derived(
        CONTENTS.map((content) => ({ content, needs: needs(content, bounds, sources ?? [], countries), unshown: unshown(content, on), shows: shows(content) })),
    );
    const terrain = $derived(needs(TERRAIN, bounds, sources ?? [], countries));
    const readers = $derived(CONTENTS.filter((content) => content.terrain && on.includes(content.id)).map((content) => content.title));
    const blocked = $derived(cards.filter(({ content, needs }) => on.includes(content.id) && !needs.ready));
    const args = $derived(planArgs(to, [[area.flag, area.value], ...layerFlags(on), ...extra]));
    const toggle = (id: string, checked: boolean) => change(toggled(on, id, checked));
    const prepare = (source: string) => prepareHash(source, area);
    const country = new Intl.DisplayNames(["en"], { type: "region" });
    /** `Switzerland and Germany`: the countries of the national models of reference `id`. */
    const modelled = (id: string) => list([...new Set(modelsOf(id, sources ?? []).map((at) => at.country))].map((code) => country.of(code) ?? code));
</script>

{#if sources}
    <div class="terrain">
        <b>Terrain here</b>
        {#if bounds && !terrain.models.length}
            <span class="soft">Copernicus 30 m: no national terrain model covers this area.</span>
        {:else if terrain.reference?.version}
            {@const covers = terrain.reference.regions.length ? ` · covers ${terrain.reference.regions.join(", ")}` : ""}
            <span class="ready">
                {`National LiDAR models · ${terrain.reference.version.version} in the archive${covers}.`}
                Where it has no tile, the bake uses Copernicus 30 m, and the review says where.
                <a href={prepare("terrain")}>Add this area</a>
            </span>
        {:else}
            <span class="stale">No national LiDAR models yet, so the bake uses Copernicus 30 m. <a href={prepare("terrain")}>Add this area</a></span>
        {/if}
        <span class="soft">{readers.length ? `Read by ${new Intl.ListFormat("en").format(readers)}.` : "Read by the bike computer maps, the planner map, Trail View and Sun."}</span>
    </div>
{/if}

<div class="cards">
    {#each cards as { content, needs, unshown, shows } (content.id)}
        {@const checked = on.includes(content.id)}
        {@const closed = !checked && (!needs.ready || !!unshown)}
        <label class="card-item" class:on={checked} class:off={closed}>
            <span class="top">
                <input type="checkbox" {checked} disabled={closed} onchange={(event) => toggle(content.id, event.currentTarget.checked)} />
                <b>{content.title}</b>
            </span>
            <span class="about">{content.about}</span>
            {#if unshown}<span class="soft note">{unshown}</span>{/if}
            {#if shows.length}<span class="soft note">Shows {new Intl.ListFormat("en").format(shows)}</span>{/if}
            {#if !sources}
                <span class="soft">Checking what it needs here…</span>
            {:else}
                {#each needs.credentials as source (source)}
                    <span class="missing">Needs the login for {source}. <a href="#/machine">Credentials</a></span>
                {/each}
                {#if content.reference && bounds}
                    {@const { source, required, without } = content.reference}
                    {@const about = referenceOf(source)}
                    {#if about?.models && !needs.models.length}
                        <span class={required ? "missing" : "soft"}>
                            No {about?.models?.noun} covers this area{required ? "" : `: uses ${without}`}. Models exist for {modelled(source) || "no country yet"}.
                        </span>
                    {:else if needs.reference?.version}
                        {@const covers = needs.reference.regions.length ? ` · covers ${needs.reference.regions.join(", ")}` : ""}
                        <span class="ready">
                            {about?.title} · {needs.reference.version.version} in the archive{covers}.
                            <a href={prepare(source)}>Add this area</a>
                        </span>
                    {:else}
                        <span class={required ? "missing" : "stale"}>
                            {about?.title} · not prepared{required ? "" : `, uses ${without}`}. <a href={prepare(source)}>Prepare this area</a>
                        </span>
                    {/if}
                {/if}
            {/if}
        </label>
    {/each}
</div>

{#if !on.length}<p class="soft">Switch on what to bake.</p>{/if}
{#if blocked.length}
    <p class="missing">{blocked.map(({ content }) => content.title).join(", ")} cannot bake here: switch {blocked.length === 1 ? "it" : "them"} off, or fix what {blocked.length === 1 ? "it needs" : "they need"}.</p>
{/if}
{#if locked}<p class="missing">A bake runs now. Plan the next one when it ends.</p>{/if}
<Action
    label="Make the plan"
    {args}
    primary
    folded
    disabled={!sources || !on.length || !!blocked.length || !!locked}
    ondone={(started: Started) => planned(started.run)}
/>
<p class="soft">The plan downloads what it needs to count the work, then shows the cost. It changes nothing yet.</p>

<style>
    .terrain {
        display: flex;
        flex-direction: column;
        gap: 2px;
        padding: 12px 16px;
        border: 1px solid var(--line-strong);
        border-radius: 10px;
    }
    .cards {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
        gap: 12px;
    }
    .card-item {
        display: flex;
        flex-direction: column;
        gap: 6px;
        padding: 14px 16px;
        border: 1px solid var(--line-strong);
        border-radius: 10px;
        cursor: pointer;
    }
    .card-item.on {
        border-color: var(--ink);
        box-shadow: inset 0 0 0 1px var(--ink);
    }
    .card-item.off {
        cursor: not-allowed;
        background: var(--parchment);
    }
    .top {
        display: flex;
        align-items: center;
        gap: 10px;
    }
    input[type="checkbox"] {
        width: 20px;
        height: 20px;
        margin: 0;
        accent-color: var(--ink);
    }
    input:focus-visible {
        outline: 2px solid var(--running);
        outline-offset: 2px;
    }
    .about {
        color: var(--ink-soft);
    }
    .card-item :is(.missing, .stale, .ready, .note) {
        font-size: 13px;
        line-height: 19px;
    }
</style>
