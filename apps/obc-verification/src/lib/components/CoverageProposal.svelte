<script lang="ts">
  import type { Catalog, CoverageProposalReview, Requirement } from '$lib/types';
  import { coverageSummary, planCovered } from '$lib/coverage';
  import { date } from './api';
  import CoverageChanges from './CoverageChanges.svelte';
  export let proposal: CoverageProposalReview;
  export let requirement: Requirement;
  export let catalog: Catalog;
  export let busy: boolean;
  export let dirty: boolean;
  export let ondecide: (id: string, accept: boolean, feedback: string) => void;
  let rejecting = false;
  let feedback = '';
  $: from = coverageSummary(requirement).label;
  $: to = planCovered(proposal.plan) ? 'Covered' : 'Partial';
</script>
<article class="proposal" aria-label={`Coverage proposal for ${proposal.requirementId}`}>
  <div class="row">
    <div><div class="eyebrow">Proposed by {proposal.author}</div><h3><span class="badge">{from}</span> <span class="arrow" aria-hidden="true">→</span> <span class="badge" class:success={to === 'Covered'} class:warning={to === 'Partial'}>{to}</span></h3></div>
    <span class="small muted">{date(proposal.createdAt)} · commit <code>{proposal.sourceSha.slice(0, 10)}</code></span>
  </div>
  <CoverageChanges {requirement} plan={proposal.plan} {catalog} />
  {#if proposal.conflict}<p class="alert warning">{proposal.conflict}</p>{/if}
  {#if rejecting}
    <label>Feedback for the agent<textarea rows={2} maxlength={5000} bind:value={feedback} placeholder="What should change?"></textarea></label>
    <div class="actions"><button class="danger-button" disabled={busy} on:click={() => ondecide(proposal.id, false, feedback)}>Reject proposal</button><button disabled={busy} on:click={() => rejecting = false}>Keep reviewing</button></div>
  {:else}
    <div class="actions"><button class="primary" disabled={busy || dirty || !!proposal.conflict} on:click={() => ondecide(proposal.id, true, '')}>Approve</button><button disabled={busy} on:click={() => rejecting = true}>Reject…</button><span class="small muted">{dirty ? 'Save or discard your draft before approving.' : 'Approval records your review and applies the test links above.'}</span></div>
  {/if}
</article>
<style>
  .proposal { padding: 16px 18px; margin: 12px 0 18px; border: 1px solid #e8d3c8; border-left: 4px solid var(--coral); border-radius: 8px; background: #fffaf7; }
  h3 { display: flex; align-items: center; gap: 6px; margin-top: 4px; }
  .arrow { color: var(--muted); }
  .actions { margin-top: 14px; }
</style>
