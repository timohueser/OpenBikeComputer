<script lang="ts">
  import type { Requirement } from '$lib/types';
  import { coverageSummary } from '$lib/coverage';
  export let requirement: Requirement;
  $: summary = coverageSummary(requirement);
  $: state = summary.state;
</script>
<span class="coverage-badge {state}" title={state === 'covered' ? 'Every criterion has reviewed evidence' : state === 'partial' ? 'Reviewed, but gaps remain' : state === 'needs-review' ? 'Saved under the earlier workflow; the next save approves it' : 'No coverage plan yet'}>
  <span class="dot" aria-hidden="true"></span>{summary.label}{#if summary.total}<span class="count">{summary.covered}/{summary.total}</span>{/if}
</span>
<style>
  .coverage-badge { display: inline-flex; align-items: center; gap: 6px; padding: 3px 9px 3px 7px; border-radius: 999px; font-size: 11px; font-weight: 600; line-height: 1.5; white-space: nowrap; background: var(--soft); color: var(--muted); }
  .dot { width: 7px; height: 7px; border-radius: 50%; background: currentColor; opacity: .7; }
  .count { font-weight: 500; opacity: .8; }
  .covered { background: #e5eee2; color: var(--forest); }
  .partial { background: #f8efdc; color: var(--amber); }
  .needs-review { background: #fdeae4; color: #a1452f; }
</style>
