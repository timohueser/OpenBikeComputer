<script lang="ts">
  import type { Catalog, CoveragePlan, Requirement, VerificationTest } from '$lib/types';
  import CoverageCriterion from './CoverageCriterion.svelte';
  export let plan: CoveragePlan;
  export let requirement: Requirement;
  export let catalog: Catalog | undefined = undefined;
  /** Passed through to each criterion by the release view. */
  export let result: ((test: VerificationTest) => { outcome: string; detail?: string; label?: string; disabled?: boolean; onrun?: () => void }) | undefined = undefined;
</script>
<!-- The rationale is written for whoever decides on the plan: the scope of the audit and its
     conclusion, including what was looked for and not found. Once the plan is approved it is a
     record rather than a question, so it is folded away — the criteria are what a reader of an
     approved requirement is here for. `CoverageChanges` keeps it open, because there it is the
     thing being judged. -->
{#if plan.rationale}<details class="small rationale"><summary>Why this plan</summary><p class="wrap">{plan.rationale}</p></details>{/if}
<div class="criteria">{#each plan.criteria as criterion, index (criterion.id)}<CoverageCriterion {criterion} number={index + 1} {requirement} {catalog} {result} />{/each}</div>
<style>.rationale { margin: 4px 0 12px; } .rationale p { margin: 6px 0 0; } .criteria { display: flex; flex-direction: column; gap: 8px; }</style>
