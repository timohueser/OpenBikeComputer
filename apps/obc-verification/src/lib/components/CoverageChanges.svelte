<script lang="ts">
  import type { Catalog, CoveragePlan, Requirement } from '$lib/types';
  import { coverageChanges } from '$lib/coverage';
  import CoverageCriterion from './CoverageCriterion.svelte';
  export let requirement: Requirement;
  export let plan: CoveragePlan;
  export let catalog: Catalog;
  $: before = requirement.coverage?.criteria ?? [];
  $: delta = coverageChanges(requirement, plan);
  $: touched = new Set([...delta.added, ...delta.changed].map(c => c.id));
  $: unchanged = plan.criteria.filter(c => !touched.has(c.id));
</script>
<p class="wrap rationale">{plan.rationale}</p>
{#if requirement.coverage && requirement.coverage.rationale !== plan.rationale}<details class="small"><summary>Previous explanation</summary><p class="wrap">{requirement.coverage.rationale}</p></details>{/if}
<div class="criteria">
  {#each plan.criteria.filter(c => touched.has(c.id)) as criterion (criterion.id)}
    {@const old = before.find(o => o.id === criterion.id)}
    <CoverageCriterion {criterion} {requirement} {catalog} previous={old} tag={old ? 'Changed' : 'New'} />
  {/each}
  {#each delta.removed as criterion (criterion.id)}<CoverageCriterion {criterion} {requirement} {catalog} removed tag="Removed" />{/each}
</div>
{#if !touched.size && !delta.removed.length}<p class="muted small">No criteria change. Approval renews the review with this explanation.</p>{/if}
{#if unchanged.length}<details class="small unchanged"><summary>{unchanged.length} unchanged {unchanged.length === 1 ? 'criterion' : 'criteria'}</summary><div class="criteria">{#each unchanged as criterion (criterion.id)}<CoverageCriterion {criterion} {requirement} {catalog} />{/each}</div></details>{/if}
<style>
  .rationale { margin: 8px 0 12px; }
  .criteria { display: flex; flex-direction: column; gap: 8px; }
  .unchanged { margin-top: 10px; }
</style>
