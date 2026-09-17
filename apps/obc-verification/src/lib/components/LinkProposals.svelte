<script lang="ts">
  import type { ProposalReview } from '$lib/types';
  import { date } from './api';
  import Markdown from './Markdown.svelte';
  export let proposals: ProposalReview[];
  export let busy: boolean;
  export let dirty: boolean;
  export let ondecide: (id: string, accept: boolean) => void;
  export let onrefresh: () => void;
  export let onclose: () => void;
  let search = '';
  let status = 'pending';
  $: pending = proposals.filter(p => p.status === 'pending');
  $: conflicts = pending.filter(p => p.conflict).length;
  $: matching = proposals.filter(p => (status === 'all' || p.status === status) &&
    `${p.requirementId} ${p.requirement?.title ?? ''} ${p.requirement?.group ?? ''} ${p.test?.name ?? ''} ${p.caseId} ${p.reason}`.toLowerCase().includes(search.toLowerCase()));
</script>

<section class="panel" aria-label="Proposed verification links">
  <div class="row"><h2>Proposed verification links</h2><div class="actions"><button disabled={busy} on:click={onrefresh}>Refresh proposals</button><button on:click={onclose}>Close</button></div></div>
  <p class="muted small">{pending.length} pending · {pending.length - conflicts} ready for review · {conflicts} with conflicts. Approval saves the link in a new revision. It does not record a test pass.</p>
  {#if dirty}<p class="alert warning">Save or discard your draft before approving a link. You can still reject proposals.</p>{/if}
  <div class="row">
    <label>Find a proposal<input type="search" bind:value={search} placeholder="Requirement, test, or reason…" /></label>
    <label>Status<select aria-label="Proposal status" bind:value={status}><option value="pending">Pending</option><option value="accepted">Accepted</option><option value="rejected">Rejected</option><option value="all">All</option></select></label>
  </div>
  {#each matching as p (p.id)}
    <div class="test">
      <div class="row"><h3>{p.action === 'add' ? 'Link test' : 'Unlink test'} · {p.requirementId}{p.requirement ? ` · ${p.requirement.title}` : ''}</h3><span class="badge">{p.status}</span></div>
      {#if p.requirement}<details><summary>Requirement statement · {p.requirement.group || 'Ungrouped'}</summary><Markdown text={p.requirement.statement} /></details>{/if}
      {#if p.test}<p><strong>{p.test.name}</strong><br /><span class="small muted">{p.test.suite}{p.test.file ? ` · ${p.test.file}` : ''}</span></p>{/if}
      <code class="wrap small">{p.caseId}</code>
      <p>{p.reason}</p>
      <p class="small muted">{p.author} · {date(p.createdAt)} · proposed against r{p.baseRevision}</p>
      {#if p.agentToken}<p class="small muted wrap">Agent token issued by {p.agentToken.issuedBy.name} · {p.agentToken.name} · {p.agentToken.id}</p>{/if}
      {#if p.status === 'pending'}
        {#if p.conflict}<p class="warning small">{p.conflict}</p>{/if}
        <div class="actions"><button class="primary" disabled={busy || dirty || !!p.conflict} on:click={() => ondecide(p.id, true)}>{p.action === 'add' ? 'Approve link' : 'Approve unlink'}</button><button disabled={busy} on:click={() => ondecide(p.id, false)}>Reject</button></div>
      {/if}
    </div>
  {:else}<p class="muted">{search ? 'No proposals match this search.' : 'No proposals with this status.'}</p>{/each}
</section>
