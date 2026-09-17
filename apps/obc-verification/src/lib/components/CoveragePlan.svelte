<script lang="ts">
  import type { Catalog, CoveragePlan, Requirement, VerificationTest } from '$lib/types';
  import CoverageCriterion from './CoverageCriterion.svelte';
  export let plan: CoveragePlan;
  export let requirement: Requirement;
  export let catalog: Catalog | undefined = undefined;
  /** Passed through to each criterion by the release view. */
  export let result: ((test: VerificationTest) => { outcome: string; detail?: string; label?: string; disabled?: boolean; onrun?: () => void }) | undefined = undefined;
</script>
<p class="wrap rationale">{plan.rationale}</p>
<div class="criteria">{#each plan.criteria as criterion, index (criterion.id)}<CoverageCriterion {criterion} number={index + 1} {requirement} {catalog} {result} />{/each}</div>
<style>.rationale { margin: 8px 0 12px; } .criteria { display: flex; flex-direction: column; gap: 8px; }</style>
