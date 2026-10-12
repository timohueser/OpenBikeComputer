<script lang="ts">
    import { command } from "../../lib/data/cli";
    import type { Screen } from "../../lib/data/screens";

    let { screen }: { screen: Extract<Screen, { id: "sources" }> } = $props();

    const TERMINAL = { sources: ["sources"] };
</script>

<h1>{screen.label}</h1>
<p>Not built yet. Use a terminal: <code>{command(TERMINAL[screen.id])}</code></p>
