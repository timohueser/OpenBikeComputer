<script lang="ts">
    import { command } from "../../lib/data/cli";
    import type { Screen } from "../../lib/data/screens";

    let { screen }: { screen: Exclude<Screen, { id: "machine" }> } = $props();

    const LATER = {
        overview: { part: "P5-3", shows: "sources, layers and targets in one picture", args: [] },
        prod: { part: "P5-3", shows: "the area, layers and releases of prod, the review of an update, and rollback", args: [] },
        local: { part: "P5-3", shows: "the area and layers of local, and an update of local", args: [] },
        sources: { part: "P5-5", shows: "each source with its versions and refresh policy, and the prepared sources", args: ["sources"] },
        runs: { part: "P5-2", shows: "the runs on this machine and on other machines, with their phases and log", args: ["runs"] },
    };
    const later = $derived(LATER[screen.id]);
</script>

<h1>{screen.label}</h1>
<p>This screen comes in part {later.part} of the operator page. It shows {later.shows}.</p>
<p>Until then, use a terminal: <code>{command(later.args)}</code></p>

<style>
    h1 {
        font-size: 28px;
        line-height: 36px;
    }
    p {
        margin: 0;
    }
    code {
        font-family: var(--mono);
        font-size: 13px;
    }
</style>
