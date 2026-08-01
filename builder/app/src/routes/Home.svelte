<script lang="ts">
    import { onMount } from "svelte";
    import CoverageHome from "../components/coverage/CoverageHome.svelte";
    import { CatalogClient } from "../lib/catalog/client";
    import { platform } from "../lib/platform";

    let { active = true }: { active?: boolean } = $props();

    let catalog = $state<{ client: CatalogClient; body: string } | null>(null);
    let error = $state<string | null>(null);

    onMount(async () => {
        try {
            const { url, body } = await platform.catalog();
            catalog = { client: CatalogClient.fromBody(body, url, { fetchImpl: platform.catalogFetch }), body };
        } catch (cause) {
            error = cause instanceof Error ? cause.message : String(cause);
        }
    });
</script>

{#if catalog}
    <CoverageHome client={catalog.client} rootBody={catalog.body} {active} />
{:else if error}
    <p class="catalog-error small" role="alert">
        The published map catalog couldn't be read: {error}
    </p>
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
