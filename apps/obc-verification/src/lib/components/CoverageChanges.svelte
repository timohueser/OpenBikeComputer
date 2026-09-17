<script lang="ts">
  import type { Catalog, CoveragePlan, Requirement } from '$lib/types';
  import { coverageChanges, coverageStatus } from '$lib/coverage';
  import CoverageCriterion from './CoverageCriterion.svelte';
  export let requirement: Requirement;
  export let plan: CoveragePlan;
  export let catalog: Catalog;
  $: delta = coverageChanges(requirement, plan);
</script>
<div class="changes">
  <p><strong>Coverage: {coverageStatus(requirement)} → {plan.conclusion === 'complete' ? 'Complete' : 'Partial'}</strong></p>
  <p class="wrap">{plan.rationale}</p>
  {#if requirement.coverage && requirement.coverage.rationale !== plan.rationale}<details><summary>Previous assessment</summary><p class="wrap">{requirement.coverage.rationale}</p></details>{/if}
  <p class="small muted">Criteria: {delta.added.length} added · {delta.changed.length} changed · {delta.removed.length} removed. Assessed source: <code>{plan.sourceSha.slice(0, 10)}</code>.</p>
  {#if requirement.coverage && requirement.coverage.sourceSha !== plan.sourceSha}<p class="small">Source assessment changes from <code>{requirement.coverage.sourceSha.slice(0, 10)}</code> to <code>{plan.sourceSha.slice(0, 10)}</code>.</p>{/if}
  {#each delta.changed as criterion}
    <div class="comparison"><div><p class="eyebrow">Current</p><CoverageCriterion criterion={requirement.coverage!.criteria.find(c => c.id === criterion.id)!} {requirement} {catalog} /></div><div><p class="eyebrow">Proposed</p><CoverageCriterion {criterion} {requirement} {catalog} /></div></div>
  {/each}
  {#each delta.added as criterion}<p class="eyebrow">Added criterion</p><CoverageCriterion {criterion} {requirement} {catalog} />{/each}
  {#each delta.removed as criterion}<div class="alert warning"><strong>Remove criterion</strong><CoverageCriterion {criterion} {requirement} {catalog} /></div>{/each}
  {#if delta.addedCases.length}<p><strong>Tests to link ({delta.addedCases.length})</strong></p><ul>{#each delta.addedCases as id}<li class="wrap">{catalog.cases.find(c => c.id === id)?.name ?? id}</li>{/each}</ul>{/if}
  {#if delta.removedTests.length}<div class="alert warning"><strong>Tests to unlink ({delta.removedTests.length})</strong><ul>{#each delta.removedTests as t}<li class="wrap">{t.title}{t.kind === 'manual' ? ' — removes this manual procedure from the requirement' : ''}</li>{/each}</ul><p class="small">These tests will no longer be required by future candidates. Existing candidates keep their saved tests and results.</p></div>{/if}
  {#if delta.unmappedTests.length}<p class="small muted">{delta.unmappedTests.length} existing tests remain linked without a criterion mapping. They still require passing results for a release.</p>{/if}
  {#if !delta.added.length && !delta.changed.length && !delta.removed.length}<p class="muted">Criteria and evidence mappings are unchanged. Approval renews the review with this assessment.</p>{/if}
</div>
<style>.comparison { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 16px; } @media(max-width: 900px) { .comparison { grid-template-columns: minmax(0, 1fr); } }</style>
