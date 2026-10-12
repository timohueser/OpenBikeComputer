<script lang="ts">
    import { command } from "../../lib/data/cli";
    import type { Screen } from "../../lib/data/screens";

    let { screen }: { screen: Exclude<Screen, { id: "machine" }> } = $props();

    const TERMINAL = { overview: [], prod: [], local: [], sources: ["sources"], runs: ["runs"] };
</script>

<h1>{screen.label}</h1>
<p>Not built yet. Use a terminal: <code>{command(TERMINAL[screen.id])}</code></p>

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
