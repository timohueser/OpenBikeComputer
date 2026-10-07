<script lang="ts">
    import type { CoverageStore } from "../../src/lib/coverage/store.svelte";
    let { store }: { store: CoverageStore } = $props();
</script>

<button type="button" onclick={() => store.addRegion("europe/switzerland")}>Choose Switzerland</button>
<p data-catalog={store.client.baseUrl}>Catalog: {store.client.baseUrl}</p>
