<script lang="ts">
  import type { Catalog, CoveragePlan, Requirement } from '$lib/types';
  import CoverageCriterion from './CoverageCriterion.svelte';
  export let plan: CoveragePlan;
  export let requirement: Requirement;
  export let catalog: Catalog | undefined = undefined;
</script>
<p class="wrap rationale">{plan.rationale}</p>
<div class="criteria">{#each plan.criteria as criterion (criterion.id)}<CoverageCriterion {criterion} {requirement} {catalog} />{/each}</div>
<style>.rationale { margin: 8px 0 12px; } .criteria { display: flex; flex-direction: column; gap: 8px; }</style>
