<script lang="ts">
    import { CliQuery } from "../../lib/data/query.svelte";
    import { blocking, short, type DeployPlan, type Deployed, type RunList, type Running } from "../../lib/data/shapes";
    import Action from "./Action.svelte";
    import Checks from "./Checks.svelte";
    import Credential from "./Credential.svelte";
    import Failure from "./Failure.svelte";

    // The code that serves prod beside its data: what each part runs, what a deploy from this machine
    // needs, and the deploy, after its dry run and a confirm.
    let { deployed, runs }: { deployed: CliQuery<Deployed>; runs: CliQuery<RunList> } = $props();

    const PARTS = [
        { what: "services", title: "Follower on the VPS", serves: "the routing, search and downloads of each release" },
        { what: "tiles", title: "Tiles Worker", serves: "the map tiles of each release" },
    ] as const;

    // The dry run of the part that the person deploys.
    let planned = $state<CliQuery<DeployPlan> | null>(null);
    function plan(what: string) {
        planned = new CliQuery<DeployPlan>(["deploy", what, "--dry-run"]);
        void planned.refresh();
    }
    const deploying = (what: string) =>
        runs.output?.runs.find((run) => run.args[0] === "deploy" && run.args[1] === what && (run.state === "starting" || run.state === "running"));
</script>

{#snippet part(what: "services" | "tiles", title: string, serves: string, running: Running, checkout: string)}
    {@const blocked = blocking(running.checks)}
    {@const run = deploying(what)}
    <div class="part">
        <h3>{title}</h3>
        <p class={running.reason ? "missing" : ""}>
            {#if running.reason}
                ✗ {running.reason}
            {:else}
                ✓ Serves {serves}{running.commit ? ` · commit ${short(running.commit)}${running.commit === checkout ? " (this checkout)" : ""}` : ""}
            {/if}
        </p>
        <Checks checks={running.checks} onchanged={() => void deployed.refresh()} />
        <Credential credential={running.credential} set={running.credential.set ?? []} onsaved={() => void deployed.refresh()} />
        {#if run}
            <p>Run <a href="#/runs/{run.id}">{run.id}</a> deploys it now.</p>
        {:else if planned?.args[1] === what}
            {#if planned.error}<Failure error={planned.error} />{/if}
            {#if planned.output}
                {@const dry = planned.output}
                <div class="confirm">
                    <b>Deploy commit {short(dry.commit)}?</b>
                    <p>{dry.text}</p>
                    {#each dry.commands as command (command)}<code>{command}</code>{/each}
                    <div class="title">
                        <Action label="Deploy now" args={[...dry.args, "--yes"]} ondone={(started: { run: string }) => (location.hash = `#/runs/${started.run}`)} />
                        <button type="button" onclick={() => (planned = null)}>Cancel</button>
                    </div>
                </div>
            {:else if !planned.error}
                <p class="soft">Reading what the deploy does…</p>
            {/if}
        {:else}
            <div class="title">
                <button type="button" disabled={!!blocked} onclick={() => plan(what)}>Deploy {short(checkout)}…</button>
                {#if blocked}<span class="soft">Fix {blocked.name} first.</span>{/if}
            </div>
        {/if}
    </div>
{/snippet}

<section class="card" aria-labelledby="services">
    <div class="title">
        <h2 id="services">Services of prod</h2>
        {#if deployed.output}<span class="soft">this checkout {short(deployed.output.checkout)}</span>{/if}
        <span class="grow"></span>
        <Action label="Check again" query={deployed} />
    </div>
    <p class="soft">An update publishes data. The follower and the tiles Worker run code: deploy them before the first update, and when a review names them.</p>
    {#if deployed.error}<Failure error={deployed.error} />{/if}
    {#if deployed.output}
        {@const output = deployed.output}
        {#each PARTS as { what, title, serves } (what)}
            {@render part(what, title, serves, output[what], output.checkout)}
        {/each}
    {:else if !deployed.error}
        <p class="soft">Asking the follower, the Worker and the VPS…</p>
    {/if}
</section>

<style>
    .part {
        display: flex;
        flex-direction: column;
        gap: 8px;
        padding-top: 8px;
        border-top: 1px solid var(--line);
    }
    h3 {
        margin: 0;
        font-size: 15px;
    }
    .confirm {
        display: flex;
        flex-direction: column;
        gap: 8px;
        padding: 16px;
        background: var(--warn);
        border: 1px solid var(--stale);
        border-radius: 8px;
        overflow-wrap: anywhere;
    }
</style>
