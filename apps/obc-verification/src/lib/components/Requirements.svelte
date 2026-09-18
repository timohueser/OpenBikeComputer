<script lang="ts">
  import { onMount, tick } from 'svelte';
  import type { Revision, Requirement, Catalog, CoverageProposalReview, ProblemAt } from '$lib/types';
  import { ApiError, api, clone, date, message } from './api';
  import { parseRequirements, formatRequirements } from './markdown-requirements';
  import Markdown from './Markdown.svelte';
  import MarkdownField from './MarkdownField.svelte';
  import GroupPicker from './GroupPicker.svelte';
  import ProseCheck from './ProseCheck.svelte';
  import RequirementGroups from './RequirementGroups.svelte';
  import RequirementLabels from './RequirementLabels.svelte';
  import CoveragePlan from './CoveragePlan.svelte';
  import CoverageEditor from './CoverageEditor.svelte';
  import CoverageBadge from './CoverageBadge.svelte';
  import CoverageProposal from './CoverageProposal.svelte';
  import { coverageDefinition, coverageSummary, planBlank, planProblem } from '$lib/coverage';
  export let revision: Revision;
  export let catalog: Catalog;
  export let dirty = false;
  export let onsaved: (revision: Revision) => void;
  export let oncatalog: (catalog: Catalog) => void;
  let requirements: Requirement[] = clone(revision.requirements);
  let selected = requirements[0]?.id || '';
  let edit = false;
  let coverageEditing = false;
  /** True while the coverage editor holds an open manual procedure. */
  let procedureOpen = false;
  let query = '';
  let open: Record<string, boolean> = { [groupName(requirements[0])]: true };
  let manageGroups = false;
  let busy = false;
  let error = '';
  let notice = '';
  let history: Revision[] | null = null;
  let historical: Revision | null = null;
  let coverageProposals: CoverageProposalReview[] = [];
  let rejecting = false;
  let deleting = false;
  let deleted: { requirement: Requirement; index: number }[] = [];
  let titleField: HTMLInputElement;
  /** Where the server says the last failure is, and the criterion to mark once the editor is open. */
  let errorAt: ProblemAt | undefined;
  let flagged = '';
  $: dirty = procedureOpen || JSON.stringify(requirements) !== JSON.stringify(revision.requirements);
  $: requirement = requirements.find(r => r.id === selected);
  $: groups = groupOrder(requirements).filter(Boolean).map(name => ({ name, count: requirements.filter(r => groupName(r) === name).length }));
  $: filtered = requirements.filter(r => matches(r, query));
  /** Excluded requirements need no plan, so only active ones are counted. */
  $: active = requirements.filter(r => r.active);
  $: covered = active.filter(r => coverageSummary(r).state === 'covered').length;
  $: sections = groupOrder(filtered).map(name => {
    const list = filtered.filter(r => groupName(r) === name);
    const inGate = list.filter(r => r.active);
    return { name, requirements: list, active: inGate.length, covered: inGate.filter(r => coverageSummary(r).state === 'covered').length, expanded: !!query || !!open[name] };
  });
  $: pendingPlans = coverageProposals.filter(p => p.status === 'pending');
  /** Requirements with a proposal to approve, in sidebar order. */
  $: reviewQueue = requirements.filter(r => pendingPlans.some(p => p.requirementId === r.id)).map(r => r.id);
  $: reviewable = !coverageEditing && requirement ? pendingPlans.find(p => p.requirementId === requirement.id) : undefined;
  function nextReview() {
    if (!reviewQueue.length) return;
    const order = requirements.map(r => r.id);
    const at = order.indexOf(selected);
    const id = reviewQueue.find(value => order.indexOf(value) > at) ?? reviewQueue[0];
    if (query && !matches(requirements.find(r => r.id === id)!, query)) query = '';
    select(id);
  }
  function fail(e: unknown) { error = message(e); errorAt = e instanceof ApiError ? e.at : undefined; }
  $: if (!error) errorAt = undefined;
  /** Opens the requirement the failure names, and its plan when the failure is in one. */
  function showProblem() {
    const at = errorAt;
    const target = at?.requirementId ? requirements.find(r => r.id === at.requirementId) : undefined;
    if (!at || !target) return;
    if (query && !matches(target, query)) query = '';
    select(target.id);
    if (at.criterionId && target.coverage) { flagged = at.criterionId; editCoverage(); }
  }
  function groupName(r: Requirement | undefined) { return r?.group?.trim() || ''; }
  /** Groups in the order they first appear in the draft; ungrouped last. */
  function groupOrder(list: Requirement[]) {
    const names: string[] = [];
    for (const r of list) if (!names.includes(groupName(r))) names.push(groupName(r));
    return names.sort((a, b) => (a === '' ? 1 : 0) - (b === '' ? 1 : 0));
  }
  function matches(r: Requirement, term: string) { return `${r.id} ${r.title} ${r.statement} ${groupName(r)}`.toLowerCase().includes(term.toLowerCase()); }
  async function reveal(id: string) {
    const r = requirements.find(r => r.id === id);
    if (r) open = { ...open, [groupName(r)]: true };
    await tick();
    document.getElementById('entry-' + id)?.scrollIntoView({ block: 'nearest' });
  }
  function setQuery(term: string) {
    const next = requirements.filter(r => matches(r, term));
    if (selected && !next.some(r => r.id === selected)) { leaveEditor(); selected = next[0]?.id || ''; }
    query = term;
  }
  function setGroup(value: string) { if (!busy && requirement) update({ ...requirement, group: value }); }
  function setTitle(value: string) { if (!busy && requirement) update({ ...requirement, title: value }); }
  function renameGroup(name: string, replacement: string) {
    if (busy) return;
    requirements = requirements.map(r => groupName(r) === name ? { ...r, group: replacement } : r);
    open = { ...open, [replacement]: !!open[name] };
  }
  function removeGroup(name: string) {
    if (busy) return;
    requirements = requirements.map(r => groupName(r) === name ? { ...r, group: undefined } : r);
    open = { ...open, '': true };
  }
  function leaveEditor() {
    closeCoverage();
    edit = false; deleting = false; rejecting = false;
  }
  function select(id: string) { leaveEditor(); selected = id; reveal(id); }
  async function create() {
    if (busy) return;
    leaveEditor();
    busy = true; error = '';
    try {
      const { id } = await api<{ id: string }>('/api/requirements/next-id', 'POST');
      const group = groupName(requirement);
      const r: Requirement = { ...(group ? { group } : {}), id, title: '', statement: '', active: true, tests: [] };
      const at = requirement ? requirements.indexOf(requirement) + 1 : requirements.length;
      requirements = [...requirements.slice(0, at), r, ...requirements.slice(at)]; selected = r.id; query = ''; edit = true; reveal(r.id);
      await tick(); document.getElementById('requirement-title')?.focus();
    } catch (e) { fail(e); } finally { busy = false; }
  }
  /** Ctrl/Cmd+Enter in the editor finishes this requirement and starts the next one in the same group. */
  function editorKeys(event: KeyboardEvent) { if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) { event.preventDefault(); create(); } }
  function move(delta: number) {
    if (!requirement) return;
    const siblings = requirements.filter(r => groupName(r) === groupName(requirement));
    const at = siblings.indexOf(requirement), swap = siblings[at + delta];
    if (!swap) return;
    const list = [...requirements], a = list.indexOf(requirement), b = list.indexOf(swap);
    list[a] = swap; list[b] = requirement; requirements = list; reveal(requirement.id);
  }
  function removeRequirement() {
    if (busy || !requirement) return;
    deleted = [...deleted, { requirement: clone(requirement), index: requirements.indexOf(requirement) }];
    requirements = requirements.filter(r => r.id !== selected);
    selected = requirements.find(r => matches(r, query))?.id || '';
    leaveEditor();
  }
  function restoreRequirement() {
    if (busy || !deleted.length) return;
    leaveEditor();
    const item = deleted[deleted.length - 1];
    const restored = [...requirements]; restored.splice(item.index, 0, item.requirement); requirements = restored;
    deleted = deleted.slice(0, -1); query = ''; selected = item.requirement.id; reveal(selected);
  }
  function update(r: Requirement) { requirements = requirements.map(value => value.id === r.id ? r : value); }
  function editCoverage() {
    if (!requirement) return;
    if (!requirement.coverage) requirement.coverage = { rationale: '', criteria: [{ id: crypto.randomUUID(), statement: '', evidence: [], gap: '' }] };
    coverageEditing = true; edit = false; manageGroups = false; error = ''; notice = ''; requirements = [...requirements];
  }
  /** An untouched plan is dropped so the requirement stays "not assessed". */
  function closeCoverage() {
    if (coverageEditing && requirement?.coverage && planBlank(requirement.coverage)) delete requirement.coverage;
    coverageEditing = false; flagged = ''; requirements = [...requirements];
  }
  async function importMarkdown(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0]; input.value = '';
    if (!file) return;
    leaveEditor();
    error = ''; notice = '';
    const parsed = parseRequirements(await file.text());
    if (!parsed.length) { error = `No requirements found in ${file.name}. Expected lines like “- **REQ-001 — Title.** Statement” under “## Group” headings.`; return; }
    let updated = 0;
    const next = requirements.map(r => { const p = parsed.find(p => p.id === r.id); if (!p) return r; updated++; return { ...r, title: p.title, statement: p.statement, group: p.group }; });
    const fresh = parsed.filter(p => !requirements.some(r => r.id === p.id));
    busy = true;
    try {
      const { ids } = fresh.length ? await api<{ ids: string[] }>('/api/requirements/next-id', 'POST', { count: fresh.length }) : { ids: [] };
      const added: Requirement[] = fresh.map((p, i) => ({ ...p, id: ids[i], active: true, tests: [] }));
      requirements = [...next, ...added];
      if (added.length && !requirement) selected = added[0].id;
      notice = `Imported ${parsed.length} requirements from ${file.name}: ${updated} updated by matching ID, ${added.length} new with fresh IDs. Tests and labels are kept. Save revision to keep the import.`;
      if (selected) reveal(selected);
    } catch (e) { fail(e); } finally { busy = false; }
  }
  /** Compare a revision with the one saved before it. */
  function diff(target: Revision, all: Revision[]) {
    const before = all.filter(r => r.id < target.id).sort((a, b) => b.id - a.id)[0];
    const was = new Map((before?.requirements ?? []).map(r => [r.id, r]));
    const now = new Map(target.requirements.map(r => [r.id, r]));
    const canon = (v: unknown): string => JSON.stringify(v, (_, value) => value && typeof value === 'object' && !Array.isArray(value) ? Object.fromEntries(Object.entries(value).filter(([, x]) => x !== undefined && x !== false).sort()) : value);
    const fields = (a: Requirement, b: Requirement) => (['title', 'statement', 'group', 'todo', 'implementationNeeded', 'active', 'coverage'] as const).filter(k => canon(a[k] ?? null) !== canon(b[k] ?? null));
    return {
      before,
      added: target.requirements.filter(r => !was.has(r.id)),
      removed: [...was.values()].filter(r => !now.has(r.id)),
      changed: target.requirements.flatMap(r => { const old = was.get(r.id); if (!old) return []; const f = fields(old, r); const t = canon(old.tests) !== canon(r.tests); return f.length || t ? [{ r, old, fields: f, tests: t }] : []; })
    };
  }
  function exportMarkdown() {
    const blob = new Blob([formatRequirements(requirements)], { type: 'text/markdown' });
    const a = document.createElement('a'); a.href = URL.createObjectURL(blob); a.download = `requirements-r${revision.id}${dirty ? '-draft' : ''}.md`; a.click(); URL.revokeObjectURL(a.href);
  }
  async function save() {
    error = ''; notice = '';
    if (procedureOpen) { error = 'Keep or cancel the manual procedure changes before saving the revision.'; return; }
    const incomplete = requirements.find(r => !r.title.trim() || !r.statement.trim());
    if (incomplete) { error = `${incomplete.id} needs a title and statement before you save.`; select(incomplete.id); return; }
    for (const r of requirements) if (r.coverage && planBlank(r.coverage)) delete r.coverage;
    const invalid = requirements.find(r => r.coverage && planProblem(r.coverage));
    if (invalid) { error = `${invalid.id} coverage: ${planProblem(invalid.coverage!)}`; select(invalid.id); editCoverage(); return; }
    busy = true;
    try {
      const saved = await api<Revision>('/api/requirements', 'PUT', { baseRevision: revision.id, requirements });
      revision = saved; requirements = clone(saved.requirements); deleted = []; deleting = false; edit = false; onsaved(saved); notice = `Revision r${saved.id} saved. Existing candidates keep their original revision.`;
      await loadProposals();
    } catch (e) { fail(e); } finally { busy = false; }
  }
  async function refresh() {
    if (dirty && !confirm('Discard this draft and load the latest saved revision?')) return;
    busy = true; error = '';
    try { const versions = await api<Revision[]>('/api/revisions'); const latest = versions.sort((a,b) => b.id - a.id)[0]; if (latest) { revision = latest; requirements = clone(latest.requirements); query = ''; if (!requirements.some(r => r.id === selected)) selected = requirements[0]?.id || ''; coverageEditing = false; edit = false; deleted = []; deleting = false; onsaved(latest); reveal(selected); } await loadProposals(); }
    catch (e) { fail(e); } finally { busy = false; }
  }
  async function refreshCatalog() {
    busy = true; error = '';
    try { catalog = await api<Catalog>('/api/catalog'); oncatalog(catalog); } catch (e) { fail(e); } finally { busy = false; }
  }
  async function showHistory() { error = ''; try { history = await api<Revision[]>('/api/revisions'); } catch (e) { fail(e); } }
  async function loadProposals() {
    coverageProposals = await api<CoverageProposalReview[]>('/api/coverage-proposals');
  }
  onMount(() => { loadProposals().catch(e => { error = message(e); }); });
  async function decide(id: string, accept: boolean, feedback = '') {
    if (busy) return;
    if (accept && dirty) { error = 'Save or discard your draft before approving a proposal.'; return; }
    busy = true; error = '';
    try {
      const decided = await api<{ revision: Revision }>(`/api/coverage-proposals/${id}`, 'POST', { accept, feedback });
      if (accept) {
        revision = decided.revision; requirements = clone(revision.requirements); onsaved(revision);
        if (!requirements.some(r => r.id === selected)) selected = requirements[0]?.id || '';
        deleted = []; edit = false; deleting = false;
      }
      rejecting = false;
      await loadProposals();
      notice = accept ? 'Coverage approved. Existing candidates keep their original coverage review.' : 'Proposal rejected. The agent can read your feedback.';
    } catch (e) {
      error = message(e);
      try { await loadProposals(); } catch { /* Keep the original decision error. */ }
    } finally { busy = false; }
  }
  /** Ctrl/⌘+Enter approves the proposal on screen; Escape closes an open reject box. */
  function reviewKeys(event: KeyboardEvent) {
    if (event.key === 'Escape' && rejecting) { rejecting = false; return; }
    if (rejecting || event.key !== 'Enter' || !(event.metaKey || event.ctrlKey)) return;
    if ((event.target as HTMLElement | null)?.closest('input, textarea')) return;
    if (!reviewable || busy || dirty || reviewable.conflict) return;
    event.preventDefault(); decide(reviewable.id, true);
  }
</script>
<svelte:window on:keydown={(event) => { if (edit) editorKeys(event); else reviewKeys(event); }} />
<div class="page-heading row"><div><div class="eyebrow">Product verification</div><h1>Requirements</h1><p class="muted">What the product must do, and the tests that show it does.</p></div><div class="actions"><details class="menu"><summary class="button">More ▾</summary><div class="menu-list"><label class="menu-item">Import Markdown…<input class="visually-hidden" type="file" accept=".md,.markdown,text/markdown,text/plain" disabled={busy} on:change={importMarkdown} /></label><button class="menu-item" on:click={exportMarkdown}>Export Markdown</button><button class="menu-item" on:click={() => manageGroups = !manageGroups}>Manage groups</button><button class="menu-item" on:click={showHistory}>Revision history</button></div></details>{#if reviewQueue.length}<button class="review-queue" disabled={busy} title="Go to the next requirement with a proposal to review" on:click={nextReview}><span class="review-count">{reviewQueue.length}</span>{reviewQueue.length === 1 ? 'proposal to review' : 'proposals to review'}</button>{/if}<button class="primary" disabled={busy} on:click={create}>+ Requirement</button></div></div>
{#if error}<div class="alert error" role="alert"><span class="grow">{error}</span>{#if errorAt?.requirementId && requirements.some(r => r.id === errorAt?.requirementId)}<button class="text-button" disabled={busy} on:click={showProblem}>Show {errorAt.requirementId}{errorAt.criterion ? ` · criterion ${errorAt.criterion}` : ''}</button>{/if}<button class="text-button" disabled={busy} on:click={refresh}>Reload saved revision</button></div>{/if}
{#if notice}<div class="alert success" role="status">{notice}</div>{/if}
{#if deleted.length}<div class="alert warning" role="status">Deleted {deleted[deleted.length - 1].requirement.id} from this draft. Save revision to apply. <button class="text-button" disabled={busy} on:click={restoreRequirement}>Undo deletion</button></div>{/if}
{#if manageGroups}<RequirementGroups {groups} disabled={busy} onrename={renameGroup} onremove={removeGroup} onclose={() => manageGroups = false} />{/if}
{#if history !== null}
  <section class="panel"><div class="row"><h2>Revision history</h2><button on:click={() => { history = null; historical = null; }}>Close</button></div>
    <div class="history-list">{#each [...history].sort((a,b) => b.id-a.id) as r}<button class:selected={historical?.id === r.id} on:click={() => historical = r}>r{r.id} · {date(r.createdAt)} · {r.author}</button>{/each}</div>
    {#if historical}{@const d = diff(historical, history)}<div class="section"><p class="eyebrow">Revision r{historical.id}{d.before ? ` compared with r${d.before.id}` : ' · first revision'} · {historical.requirements.length} requirements</p>
      {#if !d.added.length && !d.removed.length && !d.changed.length}<p class="muted">No requirement changes in this revision.</p>{/if}
      {#each d.added as r}<div class="test"><span class="badge success">added</span> <strong>{r.id} · {r.title}</strong> <span class="small muted">· {groupName(r) || 'Ungrouped'}</span><details class="small"><summary>Statement</summary><Markdown text={r.statement} /></details></div>{/each}
      {#each d.removed as r}<div class="test"><span class="badge error">removed</span> <strong>{r.id} · {r.title}</strong> <span class="small muted">· {groupName(r) || 'Ungrouped'}</span></div>{/each}
      {#each d.changed as c}<div class="test"><span class="badge warning">changed</span> <strong>{c.r.id} · {c.r.title}</strong> <span class="small muted">· {[...c.fields, ...(c.tests ? ['tests'] : [])].join(', ')}</span>
        {#if c.fields.includes('title')}<p class="small"><del class="muted">{c.old.title}</del> → {c.r.title}</p>{/if}
        {#if c.fields.includes('group')}<p class="small">Group: <del class="muted">{groupName(c.old) || 'Ungrouped'}</del> → {groupName(c.r) || 'Ungrouped'}</p>{/if}
        {#if c.fields.some(f => f === 'todo' || f === 'implementationNeeded' || f === 'active')}<p class="small">Labels: <RequirementLabels requirement={c.old} /> → <RequirementLabels requirement={c.r} /></p>{/if}
        {#if c.fields.includes('statement')}<div class="columns"><div class="inset small"><p class="eyebrow">Before</p><Markdown text={c.old.statement} /></div><div class="inset small"><p class="eyebrow">After</p><Markdown text={c.r.statement} /></div></div>{/if}
        {#if c.tests}<p class="small muted">Tests: {c.old.tests.length} → {c.r.tests.length}{#each c.r.tests.filter(t => !c.old.tests.some(o => o.id === t.id)) as t} · +{t.title}{/each}{#each c.old.tests.filter(o => !c.r.tests.some(t => t.id === o.id)) as t} · −{t.title}{/each}</p>{/if}
      </div>{/each}
    </div>{/if}
  </section>
{/if}
<div class="workbench"><aside><div class="sidebar-scroll">
  <label class="search-label">Find a requirement<input type="search" value={query} on:input={(event) => setQuery(event.currentTarget.value)} placeholder="ID, title, text, or group…" /></label>
  <div class="muted small sidebar-caption">{query ? `${filtered.length} of ${requirements.length} match` : `${covered} of ${active.length} covered${reviewQueue.length ? ` · ${reviewQueue.length} to review` : ''}`} · r{revision.id}</div>
  {#each sections as section (section.name)}
    <button type="button" class="group-heading" aria-expanded={section.expanded} on:click={() => open = { ...open, [section.name]: !section.expanded }}><span class="chevron" aria-hidden="true">{section.expanded ? '▾' : '▸'}</span><span class="group-name">{section.name || 'Ungrouped'}</span>{#if section.active}<span class="group-count">{section.covered} of {section.active} covered</span>{/if}</button>
    {#if section.expanded}{#each section.requirements as r (r.id)}<button id={'entry-' + r.id} class="entry" class:selected={selected === r.id} on:click={() => select(r.id)}><span class="eyebrow">{r.id}</span><strong>{r.title || 'Untitled requirement'}</strong><span class="entry-status"><CoverageBadge requirement={r} />{#if reviewQueue.includes(r.id)}<span class="entry-flag proposal">proposal</span>{/if}</span><RequirementLabels requirement={r} /></button>{/each}{/if}
  {:else}<p class="muted small">{requirements.length ? 'No matching requirements.' : 'Start with one important product promise, or import a Markdown draft.'}</p>{/each}
</div></aside>
<section class="detail">
{#if requirement}
  <div class="row"><span class="eyebrow">{requirement.id} · {groupName(requirement) || 'Ungrouped'}</span><div class="actions"><RequirementLabels {requirement} />{#if !edit}<button disabled={busy} on:click={() => { deleting = false; edit = true; }}>Edit requirement</button>{/if}</div></div>
  {#if edit}<div class="section"><label>Title<input id="requirement-title" bind:this={titleField} bind:value={requirement.title} on:input={() => requirements = [...requirements]} placeholder="A clear product promise" /></label><ProseCheck text={requirement.title} language="plaintext" field={titleField} onfix={setTitle} /><GroupPicker label="Group" hint="Optional" value={requirement.group || ''} {groups} disabled={busy} onchange={setGroup} /><MarkdownField label="Requirement" bind:value={requirement.statement} on:input={() => requirements = [...requirements]} /><div class="section"><h3>Labels</h3><RequirementLabels {requirement} editable disabled={busy} onchange={update} /><p class="small muted">Click a label to apply or remove it. An incomplete definition blocks publication and cannot be excepted. Implementation needed blocks publication unless an administrator accepts a candidate exception. Excluded requirements stay visible outside release verification. Existing candidates never change.</p></div><div class="row"><div class="actions"><button on:click={() => edit = false}>Done editing</button><button class="primary" disabled={busy} title="Ctrl+Enter or ⌘+Enter" on:click={create}>Done, add next</button></div><div class="actions"><span class="small muted">Position in group</span><button class="text-button" disabled={busy} on:click={() => move(-1)}>↑ Move up</button><button class="text-button" disabled={busy} on:click={() => move(1)}>↓ Move down</button><button class="text-button danger" disabled={busy} on:click={() => deleting = !deleting}>Delete requirement</button></div></div>
    {#if deleting}<div class="alert warning"><strong>Delete {requirement.id} · {requirement.title || 'Untitled requirement'}?</strong><p>This removes the requirement and its {requirement.tests.length} tests from the draft. Save revision to apply. Existing revisions and release candidates stay unchanged.</p><div class="actions"><button class="danger" disabled={busy} on:click={removeRequirement}>Delete from draft</button><button on:click={() => deleting = false}>Keep requirement</button></div></div>{/if}
  </div>
  {:else}<h2 class="requirement-title">{requirement.title || 'Untitled requirement'}</h2><div class="requirement-statement"><Markdown text={requirement.statement} /></div>{/if}
  {@const saved = revision.requirements.find(r => r.id === requirement.id)}
  {@const changed = !saved || coverageDefinition(saved) !== coverageDefinition(requirement)}
  {@const plans = pendingPlans.filter(p => p.requirementId === requirement.id)}
  {@const decided = coverageProposals.filter(p => p.requirementId === requirement.id && p.status !== 'pending' && p.status !== 'superseded')}
  <section class="section coverage-section" aria-label="Requirement coverage">
    <div class="row"><div class="row coverage-head"><h2>Coverage</h2><CoverageBadge {requirement} /></div>{#if !coverageEditing}<button disabled={busy} on:click={editCoverage}>{requirement.coverage ? 'Edit coverage' : 'Define coverage'}</button>{/if}</div>
    {#if coverageEditing}<CoverageEditor {requirement} {catalog} {busy} flag={flagged} bind:editing={procedureOpen} onchange={() => requirements = [...requirements]} ondone={closeCoverage} onrefresh={refreshCatalog} />
    {:else}
      {#each plans as p (p.id)}<CoverageProposal proposal={p} {requirement} {catalog} {busy} {dirty} bind:rejecting ondecide={decide} />{/each}
      {#if requirement.coverage}
        {#if plans.length}<p class="eyebrow current-plan">Current plan</p>{/if}
        {#if changed}<p class="small muted">Approved when you save the revision.</p>
        {:else if requirement.coverage.review}<p class="small muted">Approved by {requirement.coverage.review.author} · {date(requirement.coverage.review.createdAt)}{#if requirement.coverage.review.sourceSha}{' · '}catalogue <code>{requirement.coverage.review.sourceSha.slice(0, 10)}</code>{/if}</p>{/if}
        <CoveragePlan plan={requirement.coverage} {requirement} {catalog} />
      {:else if !plans.length}<div class="empty coverage-empty"><h3>What would prove this requirement?</h3><p class="muted">Break it into checkable criteria, attach the tests that prove each one, and note the gaps. Define coverage to start, or wait for an agent proposal.</p></div>{/if}
      {#if decided.length}<details class="small history"><summary>Earlier proposals ({decided.length})</summary>{#each decided as p (p.id)}<p class="small wrap"><span class="badge" class:success={p.status === 'accepted'} class:error={p.status === 'rejected'}>{p.status}</span> {p.author} · {date(p.createdAt)}{#if p.decidedBy} · decided by {p.decidedBy}{/if}{#if p.feedback} · “{p.feedback}”{/if}</p>{/each}</details>{/if}
    {/if}
  </section>
{:else}<div class="empty"><h2>{requirements.length ? 'No matching requirements' : 'Make the important promises explicit.'}</h2><p class="muted">{requirements.length ? 'Clear your search or choose another group.' : 'Write a measurable requirement, then define the checks that provide evidence. You can also import a Markdown draft.'}</p><button class="primary" disabled={busy} on:click={create}>Create requirement</button></div>{/if}
</section></div>
{#if dirty}<div class="savebar row"><span class="small">{dirty ? 'You have unsaved changes' : `Saved revision r${revision.id}`} <span class="muted">· Candidates use saved revisions only.</span></span><div class="actions">{#if dirty}<button disabled={busy} on:click={refresh}>Discard draft</button>{/if}<button class="primary" disabled={busy || !dirty || procedureOpen} on:click={save}>{busy ? 'Saving…' : 'Save revision'}</button></div></div>{/if}
<style>.alert .grow { flex: 1; min-width: 200px; }</style>
