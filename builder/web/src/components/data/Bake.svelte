<script lang="ts">
    import { onMount, tick } from "svelte";
    import { boundsOf, extent, nameOf, parseBox, type Areas } from "../../lib/data/areas";
    import { CONTENTS } from "../../lib/data/contents";
    import { flagsOf, type Flags } from "../../lib/data/flags";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { bakeAt, bakeHash, bakeOf, served, type BakeRoute } from "../../lib/data/screens";
    import { list, type Status } from "../../lib/data/shapes";
    import type { SourceRow } from "../../lib/data/sources";
    import BakeArea from "./BakeArea.svelte";
    import BakeContents from "./BakeContents.svelte";
    import BakeProgress from "./BakeProgress.svelte";
    import BakeReview from "./BakeReview.svelte";
    import Icon from "../planner/PlannerIcon.svelte";
    import Failure from "./Failure.svelte";

    // A bake in four steps: the area, what it contains, the review of its plan, and the bake. The
    // URL holds every choice, so a reload or a link opens the same step.
    let { hash, status }: { hash: string; status: Status | null } = $props();

    const areas = new CliQuery<Areas>(["areas"]);
    const sources = new CliQuery<{ sources: SourceRow[] }>(["sources"]);
    onMount(() => {
        void areas.refresh();
        void sources.refresh();
    });

    const route = $derived(bakeAt(hash));
    const target = $derived(status?.targets.find((at) => at.target === route.to) ?? null);
    const machine = $derived(status?.machine.name ?? "this machine");
    const go = (next: Partial<BakeRoute>) => (location.hash = bakeHash({ ...route, ...next }));

    const STEPS = ["area", "contents", "review", "bake"] as const;
    /** An earlier step that the person opened again; a choice there closes it. */
    let reopened = $state<"area" | "contents" | null>(null);
    const step = $derived<(typeof STEPS)[number]>(route.run ? "bake" : route.plan ? "review" : (reopened ?? (route.area ? "contents" : "area")));

    const bounds = $derived.by(() => {
        if (!route.area) return null;
        if (route.area.flag === "--box") return parseBox(route.area.value);
        return areas.output ? boundsOf(route.area.value, areas.output) : null;
    });
    // The countries of a saved area or a Geofabrik area; a drawn box names none.
    const countries = $derived.by(() => {
        if (route.area?.flag !== "--region" || !areas.output) return [];
        const ids = route.area.value.split(",");
        return ids.flatMap((id) => areas.output!.regions.find((region) => region.id === id)?.countries ?? areas.output!.areas.find((area) => area.id === id)?.countries ?? []);
    });
    const areaName = $derived(
        route.area ? (route.area.flag === "--box" ? "A box on the map" : nameOf(route.area.value, areas.output)) : "",
    );
    const chosen = $derived(CONTENTS.filter((content) => route.on.includes(content.id)).map((content) => content.title));
    const destinations = $derived(status?.targets ?? []);
    const where = (kind: string) => (kind === "r2" ? "Production" : "This computer");
    // Another run holds the lock of the destination: its bake shows here.
    const busy = $derived(target?.lock && !target.lock.died && target.lock.machine === status?.machine.name ? target.lock.run : null);

    // A choice moves the focus to the next step, which opens in its place.
    async function chooseArea(area: BakeRoute["area"]) {
        reopened = null;
        go({ area, plan: null, run: null });
        await tick();
        document.getElementById("step-contents")?.focus();
    }
    function chooseContents(on: string[]) {
        go({ on, plan: null, run: null });
    }
    const extra = $derived<Flags>([
        ...(route.only ? [["--only", route.only] as [string, string]] : []),
        ...(route.refresh ? [["--refresh", route.refresh] as [string, string]] : []),
    ]);
    /** A bake opened by its run alone takes its settings from the arguments of the run. */
    function fromRun(args: string[]) {
        if (route.area) return;
        const flags = [...(target ? served(target) : []), ...flagsOf(args)];
        history.replaceState(null, "", bakeHash({ ...bakeOf(route.to, flags), plan: route.plan, run: route.run }));
        dispatchEvent(new HashChangeEvent("hashchange"));
    }
    function reopen(at: "area" | "contents") {
        reopened = at;
        if (route.plan || route.run) go({ plan: null, run: null });
    }
</script>

<div class="title">
    <h1>Bake</h1>
    <div class="segmented" role="radiogroup" aria-label="Bake for">
        {#each destinations as at (at.target)}
            <button
                type="button"
                role="radio"
                aria-checked={at.target === route.to}
                disabled={!!route.run}
                onclick={() => go({ to: at.target, plan: null, run: null })}>{where(at.kind)}</button
            >
        {/each}
    </div>
    <span class="soft">{target ? (target.kind === "r2" ? "the R2 bucket that riders read" : `a folder on ${machine}`) : ""}</span>
</div>

{#if areas.error}<Failure error={areas.error} />{/if}
{#if sources.error}<Failure error={sources.error} />{/if}
{#if busy && route.run !== busy}
    <p class="card running">A bake runs on {machine} now. <a href={bakeHash({ ...route, run: busy })}>Follow it</a></p>
{/if}

{#if extra.length && !route.run}
    <p class="card">
        {#if route.only}Bakes again only {route.only}, and what reads it.{/if}
        {#if route.refresh}Sets the refresh {route.refresh}.{/if}
        <button type="button" class="change" onclick={() => go({ only: null, refresh: null, plan: null })}>Clear</button>
    </p>
{/if}

{#snippet header(at: (typeof STEPS)[number], n: number, name: string, summary: string, change: (() => void) | null)}
    {@const state = at === step ? "now" : STEPS.indexOf(at) < STEPS.indexOf(step) ? "done" : "next"}
    <header class="step-head" data-state={state}>
        <span class="n" aria-hidden="true">{#if state === "done"}<Icon name="check" size={15} />{:else}{n}{/if}</span>
        <h2 id="step-{at}" tabindex="-1">{name}</h2>
        {#if state === "done"}<span class="summary">{summary}</span>{/if}
        <span class="grow"></span>
        {#if state === "done" && change}<button type="button" class="change" aria-label="Change the {name.toLowerCase()}" onclick={change}>Change</button>{/if}
    </header>
{/snippet}

<ol class="steps">
    <li class="step" class:now={step === "area"} aria-current={step === "area" ? "step" : undefined}>
        {@render header("area", 1, "Area", bounds ? `${areaName} · ${extent(bounds)}` : areaName, () => reopen("area"))}
        {#if step === "area"}
            <BakeArea
                areas={areas.output}
                chosen={route.area}
                keep
                served={target?.area?.boxes ?? []}
                serving={target?.region ? `${target.region === "box" ? "the box" : nameOf(target.region, areas.output)} on ${target.kind === "r2" ? "production" : "this computer"}` : null}
                choose={chooseArea}
            />
        {/if}
    </li>
    <li class="step" class:now={step === "contents"} aria-current={step === "contents" ? "step" : undefined}>
        {@render header("contents", 2, "Contents", list(chosen), () => reopen("contents"))}
        {#if step === "contents" && route.area}
            <BakeContents
                to={route.to}
                area={route.area}
                {bounds}
                {countries}
                on={route.on}
                {extra}
                sources={sources.output?.sources ?? null}
                locked={busy}
                change={chooseContents}
                planned={(run) => {
                    reopened = null;
                    go({ plan: run });
                }}
            />
        {/if}
    </li>
    <li class="step" class:now={step === "review"} aria-current={step === "review" ? "step" : undefined}>
        {@render header("review", 3, "Review", "planned", null)}
        {#if step === "review" && route.plan && target}
            <BakeReview
                plan={route.plan}
                {target}
                {machine}
                memory={status?.machine.memory_bytes ?? null}
                free={status?.machine.store_free_bytes ?? null}
                sources={sources.output?.sources ?? []}
                started={(run) => go({ run })}
            />
        {/if}
    </li>
    <li class="step" class:now={step === "bake"} aria-current={step === "bake" ? "step" : undefined}>
        {@render header("bake", 4, "Bake", "", null)}
        {#if step === "bake" && route.run}
            {#key route.run}
                <BakeProgress run={route.run} plan={route.plan} kind={target?.kind ?? "dir"} {status} again={() => go({ plan: null, run: null })} args={fromRun} />
            {/key}
        {/if}
    </li>
</ol>

<style>
    .steps {
        display: flex;
        flex-direction: column;
        margin: 0;
        padding: 0;
        list-style: none;
        background: var(--panel);
        border: 1px solid var(--line);
        border-radius: 10px;
    }
    .step {
        display: flex;
        flex-direction: column;
        gap: 16px;
        padding: 14px 20px;
    }
    .step + .step {
        border-top: 1px solid var(--line);
    }
    .step.now {
        padding: 18px 20px 22px;
    }
    .step-head {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 6px 12px;
        min-height: 36px;
    }
    .step-head h2:focus {
        outline: none;
    }
    .step-head[data-state="next"] {
        color: var(--ink-faint);
    }
    .n {
        display: inline-grid;
        flex: none;
        place-items: center;
        width: 26px;
        height: 26px;
        border: 1.5px solid currentColor;
        border-radius: 999px;
        font-size: 13px;
        font-weight: 700;
        font-variant-numeric: tabular-nums;
    }
    .step-head[data-state="now"] .n {
        background: var(--amber);
        border-color: var(--amber);
        color: var(--on-amber);
    }
    .step-head[data-state="done"] .n {
        background: var(--ready);
        border-color: var(--ready);
        color: var(--panel);
    }
    .summary {
        color: var(--ink-soft);
    }
    .grow {
        flex-grow: 1;
    }
</style>
