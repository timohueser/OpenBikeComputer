<script lang="ts">
    import { onMount } from "svelte";
    import CoverageHome from "../components/coverage/CoverageHome.svelte";
    import { CatalogClient } from "../lib/catalog/client";
    import { platform } from "../lib/platform";

    let { active = true }: { active?: boolean } = $props();

    let catalog = $state<{ client: CatalogClient; body: string } | null>(null);
    let error = $state<string | null>(null);

    let loading = $state(false);

    async function loadCatalog(refresh = false) {
        const { url, body } = await platform.catalog({ refresh });
        return { client: CatalogClient.fromBody(body, url, { fetchImpl: platform.catalogFetch }), body };
    }

    async function load(refresh = false) {
        loading = true;
        try {
            catalog = await loadCatalog(refresh);
            error = null;
        } catch (cause) {
            error = cause instanceof Error ? cause.message : String(cause);
        } finally {
            loading = false;
        }
    }

    onMount(() => { void load(); });
</script>

{#if catalog}
    <CoverageHome client={catalog.client} rootBody={catalog.body} {active} refreshCatalog={() => loadCatalog(true)} />
{:else if error}
    <p class="catalog-error small" role="alert">
        The published map catalog couldn't be read: {error}
    </p>
    <button type="button" class="btn" disabled={loading} onclick={() => void load(true)}>{loading ? "Refreshing the map catalog…" : "Refresh map catalog"}</button>
{:else}
    <p class="catalog-status small faint">Loading the map catalog…</p>
{/if}

<style>
    .catalog-error,
    .catalog-status {
        margin: 0;
        padding: 14px 2px;
    }

    .catalog-error {
        color: var(--coral);
    }
</style>
