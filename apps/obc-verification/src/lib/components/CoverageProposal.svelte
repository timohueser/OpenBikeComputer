<script lang="ts">
  import type { Catalog, CoverageProposalReview, Requirement } from '$lib/types';
  import { coverageChanges, planCoveredCount } from '$lib/coverage';
  import { date } from './api';
  import CoverageChanges from './CoverageChanges.svelte';
  export let proposal: CoverageProposalReview;
  export let requirement: Requirement;
  export let catalog: Catalog;
  export let busy: boolean;
  export let dirty: boolean;
  export let ondecide: (id: string, accept: boolean, feedback: string) => void;
  /** Bound by the parent so that Escape can close the reject box. */
  export let rejecting = false;
  let feedback = '';
  $: covered = planCoveredCount(proposal.plan);
  $: deleted = coverageChanges(requirement, proposal.plan).deletedProcedures;
</script>
<article class="proposal" aria-label={`Coverage proposal for ${proposal.requirementId}`}>
  <div class="row">
    <div><div class="eyebrow">Proposed by {proposal.author}</div><h3>{covered} of {proposal.plan.criteria.length} criteria covered after approval</h3></div>
    <span class="small muted">{date(proposal.createdAt)} · commit <code>{proposal.sourceSha.slice(0, 10)}</code></span>
  </div>
  <CoverageChanges {requirement} plan={proposal.plan} {catalog} />
  {#if proposal.conflict}<p class="alert warning">{proposal.conflict}</p>{/if}
  {#if deleted.length}<p class="small deletes">Approval deletes {deleted.length} manual {deleted.length === 1 ? 'procedure' : 'procedures'}, with {deleted.length === 1 ? 'its' : 'their'} steps and input files: {deleted.map(t => t.title).join(', ')}.</p>{/if}
  {#if rejecting}
    <label>Feedback for the agent<textarea rows={2} maxlength={5000} bind:value={feedback} placeholder="What should change?"></textarea></label>
    <div class="actions"><button class="danger-button" disabled={busy} on:click={() => ondecide(proposal.id, false, feedback)}>Reject proposal</button><button disabled={busy} on:click={() => rejecting = false}>Keep reviewing</button></div>
  {:else}
    <div class="actions"><button class="primary" disabled={busy || dirty || !!proposal.conflict} on:click={() => ondecide(proposal.id, true, '')}>Approve</button><button disabled={busy} on:click={() => rejecting = true}>Reject…</button><span class="small muted">{dirty ? 'Save or discard your draft before approving.' : 'Approval records your review and applies the evidence above.'}</span></div>
  {/if}
</article>
<style>
  .proposal { padding: 16px 18px; margin: 12px 0 18px; border: 1px solid #e8d3c8; border-left: 4px solid var(--coral); border-radius: 8px; background: #fffaf7; }
  h3 { margin-top: 4px; }
  .deletes { margin: 12px 0 0; color: var(--amber); }
  .actions { margin-top: 14px; }
</style>
