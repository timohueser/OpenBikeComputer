<script lang="ts">
    import type { Bbox } from "../../lib/data/areas";
    import { CONTENTS, layerFlags, needs, shows, toggled, unshown } from "../../lib/data/contents";
    import { planArgs } from "../../lib/data/flags";
    import type { BakeRoute } from "../../lib/data/screens";
    import type { Started } from "../../lib/data/shapes";
    import type { SourceRow } from "../../lib/data/sources";
    import Action from "./Action.svelte";

    // What the bake makes: one card for each part. A card says what it needs in the area and which
    // card shows it; one that cannot bake here, or that nothing on shows, says why and cannot be on.
    let {
        to,
        area,
        bounds,
        countries,
        on,
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
        /** `null` until `obc data sources` answers. */
        sources: SourceRow[] | null;
        locked: string | null;
        change: (on: string[]) => void;
        planned: (run: string) => void;
    } = $props();

    const cards = $derived(
        CONTENTS.map((content) => ({ content, needs: needs(content, bounds, sources ?? [], countries), unshown: unshown(content, on), shows: shows(content) })),
    );
    const blocked = $derived(cards.filter(({ content, needs }) => on.includes(content.id) && !needs.ready));
    const args = $derived(planArgs(to, [[area.flag, area.value], ...layerFlags(on)]));
    const toggle = (id: string, checked: boolean) => change(toggled(on, id, checked));
    const prepare = (source: string) =>
        `#/sources/${encodeURIComponent(source)}/prepare?to=local${area.flag === "--region" ? `&region=${encodeURIComponent(area.value)}` : ""}`;
    const where = (target: string) => (target === "prod" ? "on R2" : "on this computer");
</script>

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
                {#each needs.references as reference (reference.source)}
                    {#if reference.version}
                        <span class="ready">{reference.use} · prepared {reference.version.version} {where(reference.version.target)}</span>
                    {:else}
                        <span class={content.reference?.required ? "missing" : "stale"}>
                            {reference.use} · not prepared{content.reference?.required ? "" : `, uses ${content.reference?.without}`}.
                            <a href={prepare(reference.source)}>Prepare</a>
                        </span>
                    {/if}
                {/each}
                {#if content.reference?.required && bounds && !needs.references.length}
                    <span class="missing">No {content.reference.family} covers this area.</span>
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
