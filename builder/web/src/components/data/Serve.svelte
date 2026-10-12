<script lang="ts">
    import type { CliQuery } from "../../lib/data/query.svelte";
    import type { Listening, RunList } from "../../lib/data/shapes";
    import Action from "./Action.svelte";

    // `obc data serve` of a dir target as a run: Serve starts it, Stop stops that run.
    let { name, serve, runs }: { name: string; serve: Listening; runs: CliQuery<RunList> } = $props();

    // One serve at a time: it has one port.
    const serving = $derived(
        runs.output?.runs.find((run) => run.args[0] === "serve" && (run.state === "starting" || run.state === "running")) ?? null,
    );
</script>

<section class="card" aria-labelledby="serve">
    <h2 id="serve">Serve {name}</h2>
    {#if serving && serving.args[1] !== name}
        <p>Run <a href="#/runs/{serving.id}">{serving.id}</a> serves {serving.args[1]}. One target is served at a time.</p>
    {:else if serving}
        <p>
            Run <a href="#/runs/{serving.id}">{serving.id}</a> serves {name} at <code>{serve.url}</code>{#if serve.network}, and to the
                network at <code>{serve.network}</code>{/if}. Its log says when the services answer.
        </p>
        {#each serve.links as link (link.app)}<p><b>{link.app}</b> · <code>{link.how}</code></p>{/each}
        <Action label="Stop serving" args={["runs", serving.id, "--stop"]} ondone={() => void runs.refresh()} />
    {:else}
        <p>Serve gives {name} to the apps of this machine and of its network, and follows each update.</p>
        <Action label="Serve {name}" args={["serve", name, "--detach"]} ondone={() => void runs.refresh()} />
    {/if}
</section>
