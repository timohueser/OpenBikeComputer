<script lang="ts">
    import { onMount } from "svelte";
    import { boundsOf, extent, nameOf, parseBox, type Areas } from "../../lib/data/areas";
    import { cardsOf, CONTENTS } from "../../lib/data/contents";
    import { flagsOf } from "../../lib/data/flags";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { bakeAt, bakeHash, type BakeRoute } from "../../lib/data/screens";
    import { list, type Status } from "../../lib/data/shapes";
    import type { SourceRow } from "../../lib/data/sources";
    import BakeArea from "./BakeArea.svelte";
    import BakeContents from "./BakeContents.svelte";
    import BakeProgress from "./BakeProgress.svelte";
    import BakeReview from "./BakeReview.svelte";
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
        route.area ? (route.area.flag === "--box" ? `Box ${route.area.value}` : nameOf(route.area.value, areas.output)) : "",
    );
    const chosen = $derived(CONTENTS.filter((content) => route.on.includes(content.id)).map((content) => content.title));
    const destinations = $derived(status?.targets ?? []);
    const where = (kind: string) => (kind === "r2" ? "Production" : "This computer");
    // Another run holds the lock of the destination: its bake shows here.
    const busy = $derived(target?.lock && !target.lock.died && target.lock.machine === status?.machine.name ? target.lock.run : null);

    function chooseArea(area: BakeRoute["area"]) {
        reopened = null;
        go({ area, plan: null, run: null });
    }
    function chooseContents(on: string[]) {
        go({ on, plan: null, run: null });
    }
    /** A bake opened by its run alone takes its area and contents from the arguments of the run. */
    function fromRun(args: string[]) {
        if (route.area) return;
        const flags = flagsOf(args);
        const area = flags.find(([flag]) => flag === "--region" || flag === "--box");
        const on = cardsOf(flags.filter(([flag]) => flag === "--add").map(([, layer]) => layer));
        history.replaceState(null, "", bakeHash({ ...route, area: area ? { flag: area[0] as "--region" | "--box", value: area[1] } : null, on }));
        dispatchEvent(new HashChangeEvent("hashchange"));
    }
    function reopen(at: "area" | "contents") {
        reopened = at;
        if (route.plan || route.run) go({ plan: null, run: null });
    }
</script>

<div class="title">
    <h1>Bake</h1>
    <div class="destination" role="radiogroup" aria-label="Bake for">
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

{#snippet header(at: (typeof STEPS)[number], n: number, name: string, summary: string, change: (() => void) | null)}
    {@const state = at === step ? "now" : STEPS.indexOf(at) < STEPS.indexOf(step) ? "done" : "next"}
    <header class="step-head" data-state={state}>
        <span class="n" aria-hidden="true">{state === "done" ? "✓" : n}</span>
        <h2>{name}</h2>
        {#if state === "done"}<span class="summary">{summary}</span>{/if}
        <span class="grow"></span>
        {#if state === "done" && change}<button type="button" class="change" onclick={change}>Change</button>{/if}
    </header>
{/snippet}

<ol class="steps">
    <li class="step" class:now={step === "area"} aria-current={step === "area" ? "step" : undefined}>
        {@render header("area", 1, "Area", bounds ? `${areaName} · ${extent(bounds)}` : areaName, () => reopen("area"))}
        {#if step === "area"}
            <BakeArea areas={areas.output} chosen={route.area} {bounds} served={target?.area?.boxes ?? []} choose={chooseArea} />
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
    .destination {
        display: inline-flex;
        border: 1px solid var(--ink);
        border-radius: 8px;
        overflow: hidden;
    }
    .destination button {
        min-height: 36px;
        border: 0;
        border-radius: 0;
        font-weight: 500;
    }
    .destination button[aria-checked="true"] {
        background: var(--ink);
        color: var(--panel);
        font-weight: 600;
    }
    .steps {
        display: flex;
        flex-direction: column;
        gap: 12px;
        margin: 0;
        padding: 0;
        list-style: none;
    }
    .step {
        display: flex;
        flex-direction: column;
        gap: 16px;
        padding: 16px 20px;
        background: var(--panel);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
    }
    .step.now {
        padding: 20px;
        border: 2px solid var(--ink);
    }
    .step-head {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 8px 12px;
    }
    .step-head[data-state="next"] {
        color: var(--ink-faint);
    }
    .n {
        display: inline-grid;
        place-items: center;
        width: 28px;
        height: 28px;
        border: 1.5px solid currentColor;
        border-radius: 999px;
        font-weight: 600;
        font-variant-numeric: tabular-nums;
    }
    .step-head[data-state="now"] .n {
        background: var(--ink);
        border-color: var(--ink);
        color: var(--panel);
    }
    .step-head[data-state="done"] .n {
        background: var(--ready);
        border-color: var(--ready);
        color: var(--panel);
    }
    .summary {
        color: var(--ink-soft);
    }
    .change {
        min-height: 36px;
        padding: 0 14px;
        font-weight: 500;
    }
    .grow {
        flex-grow: 1;
    }
</style>
