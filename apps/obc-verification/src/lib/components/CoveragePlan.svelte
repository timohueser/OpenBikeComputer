<script lang="ts">
  import type { Catalog, CoveragePlan, Requirement } from '$lib/types';
  import CoverageCriterion from './CoverageCriterion.svelte';
  export let plan: CoveragePlan;
  export let requirement: Requirement;
  export let catalog: Catalog | undefined = undefined;
</script>
<p class="small muted">Assessed source <code>{plan.sourceSha.slice(0, 10)}</code> · {plan.criteria.length} acceptance criteria · assessment: <strong>{plan.conclusion}</strong></p>
<p class="wrap">{plan.rationale}</p>
{#each plan.criteria as criterion (criterion.id)}<CoverageCriterion {criterion} {requirement} {catalog} />{/each}
