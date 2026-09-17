<script lang="ts">
  import type { Catalog, CoverageProposalReview, ProposalReview } from '$lib/types';
  import { date } from './api';
  import Markdown from './Markdown.svelte';
  import CoverageReview from './CoverageReview.svelte';
  export let proposals: ProposalReview[];
  export let coverageProposals: CoverageProposalReview[] = [];
  export let catalog: Catalog;
  export let oncoverage: (id: string, accept: boolean, feedback: string) => void;
  export let busy: boolean;
  export let dirty: boolean;
  export let ondecide: (id: string, accept: boolean) => void;
  export let onrefresh: () => void;
  export let onclose: () => void;
  let search = '';
  let status = 'pending';
  $: coverageMatching = coverageProposals.filter(p => (status === 'all' || p.status === status) && `${p.requirementId} ${p.requirement?.title ?? ''} ${p.plan.rationale} ${p.plan.criteria.map(c => c.statement).join(' ')}`.toLowerCase().includes(search.toLowerCase()));
  $: pending = proposals.filter(p => p.status === 'pending');
  $: conflicts = pending.filter(p => p.conflict).length;
  $: matching = proposals.filter(p => (status === 'all' || p.status === status) &&
    `${p.requirementId} ${p.requirement?.title ?? ''} ${p.requirement?.group ?? ''} ${p.test?.name ?? ''} ${p.caseId} ${p.reason}`.toLowerCase().includes(search.toLowerCase()));
</script>

<section class="panel" aria-label="Coverage proposals">
  <div class="row"><h2>Coverage proposals</h2><div class="actions"><button disabled={busy} on:click={onrefresh}>Refresh proposals</button><button on:click={onclose}>Close</button></div></div>
  <p class="muted small">Review changes to criteria, evidence, and gaps together. Approval applies the displayed test-link changes; it does not record a test pass.</p>
  {#if dirty}<p class="alert warning">Save or discard your draft before approving a proposal. You can still reject proposals.</p>{/if}
  <div class="row">
    <label>Find a proposal<input type="search" bind:value={search} placeholder="Requirement, test, or reason…" /></label>
    <label>Status<select aria-label="Proposal status" bind:value={status}><option value="pending">Pending</option><option value="accepted">Accepted</option><option value="rejected">Rejected</option><option value="all">All</option></select></label>
  </div>
  <CoverageReview proposals={coverageMatching} {catalog} {busy} {dirty} ondecide={oncoverage} />
  <details><summary>Individual link suggestions ({pending.length} pending)</summary>
  <p class="small muted">Prefer coverage proposals so evidence and gaps are reviewed together. These older link-only suggestions do not assess coverage. Approving a matching coverage plan resolves them automatically. {conflicts} suggestions have conflicts.</p>
  {#each matching as p (p.id)}
    <div class="test">
      <div class="row"><h3>{p.action === 'add' ? 'Link test' : 'Unlink test'} · {p.requirementId}{p.requirement ? ` · ${p.requirement.title}` : ''}</h3><span class="badge">{p.status}</span></div>
      {#if p.requirement}<details><summary>Requirement statement · {p.requirement.group || 'Ungrouped'}</summary><Markdown text={p.requirement.statement} /></details>{/if}
      {#if p.test}<p><strong>{p.test.name}</strong><br /><span class="small muted">{p.test.suite}{p.test.file ? ` · ${p.test.file}` : ''}</span></p>{/if}
      <code class="wrap small">{p.caseId}</code>
      <p>{p.reason}</p>
      {#if p.resolvedByCoverage}<p class="small muted">Included in an approved coverage plan.</p>{/if}
      <p class="small muted">{p.author} · {date(p.createdAt)} · proposed against r{p.baseRevision}</p>
      {#if p.agentToken}<p class="small muted wrap">Agent token issued by {p.agentToken.issuedBy.name} · {p.agentToken.name} · {p.agentToken.id}</p>{/if}
      {#if p.status === 'pending'}
        {#if p.conflict}<p class="warning small">{p.conflict}</p>{/if}
        <div class="actions"><button class="primary" disabled={busy || dirty || !!p.conflict} on:click={() => ondecide(p.id, true)}>{p.action === 'add' ? 'Approve link' : 'Approve unlink'}</button><button disabled={busy} on:click={() => ondecide(p.id, false)}>Reject</button></div>
      {/if}
    </div>
  {:else}<p class="muted">{search ? 'No proposals match this search.' : 'No proposals with this status.'}</p>{/each}
  </details>
</section>
