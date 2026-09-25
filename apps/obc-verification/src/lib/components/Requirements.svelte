<script lang="ts">
  import { onMount, tick } from 'svelte';
  import type { Revision, Requirement, Catalog, CoverageProposalReview, ProblemAt, RequirementSuggestionReview } from '$lib/types';
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
  import Suggestions from './Suggestions.svelte';
  import { coverageDefinition, coverageSummary, linkEvidence, planBlank, planProblem } from '$lib/coverage';
  import { FILTERS, type Filter } from '$lib/filters';
  export let revision: Revision;
  export let catalog: Catalog;
  export let dirty = false;
  export let onsaved: (revision: Revision) => void;
  export let oncatalog: (catalog: Catalog) => void;
  let requirements: Requirement[] = clone(revision.requirements);
  /** Bound by the page, which keeps it and the filter in the URL. */
  export let selected = '';
  export let filter: Filter = 'all';
  /** False while another view is on screen; the review keys are then off. */
  export let visible = true;
  if (!requirements.some(r => r.id === selected)) selected = requirements[0]?.id || '';
  let edit = false;
  let coverageEditing = false;
  /** True while the coverage editor holds an open manual procedure. */
  let procedureOpen = false;
  let query = '';
  let open: Record<string, boolean> = { [groupName(requirements.find(r => r.id === selected))]: true };
  let manageGroups = false;
  let busy = false;
  let error = '';
  let notice = '';
  let history: Revision[] | null = null;
  let historical: Revision | null = null;
  let coverageProposals: CoverageProposalReview[] = [];
  let suggestions: RequirementSuggestionReview[] = [];
  let suggestionsOpen = false;
  /** The suggestion whose body the panel shows. */
  let expandedSuggestion = '';
  let rejecting = false;
  let deleting = false;
  let deleted: { requirement: Requirement; index: number }[] = [];
  let titleField: HTMLInputElement;
  /** Proposals approved into this draft. The save records them; discarding leaves them pending. */
  let accepted: string[] = [];
  /** Open suggestions ticked for acceptance. The save accepts them with the revision. */
  let stagedSuggestions: string[] = [];
  /** Where the server says the last failure is, and the criterion to mark once the editor is open. */
  let errorAt: ProblemAt | undefined;
  let flagged = '';
  /** The component renders only in the browser, after the workspace loads. */
  const mod = /Mac|iPhone|iPad/.test(navigator.platform) ? '⌘' : 'Ctrl+';
  $: dirty = procedureOpen || stagedSuggestions.length > 0 || JSON.stringify(requirements) !== JSON.stringify(revision.requirements);
  $: requirement = requirements.find(r => r.id === selected);
  $: groups = groupOrder(requirements).filter(Boolean).map(name => ({ name, count: requirements.filter(r => groupName(r) === name).length }));
  $: searched = requirements.filter(r => matches(r, query));
  $: shows = {
    all: () => true,
    unassessed: (r: Requirement) => coverageSummary(r).state === 'unassessed',
    partial: (r: Requirement) => coverageSummary(r).state === 'partial',
    proposal: (r: Requirement) => reviewQueue.includes(r.id),
    suggestion: (r: Requirement) => changeSuggestions.some(s => s.requirementId === r.id)
  } satisfies Record<Filter, (r: Requirement) => boolean>;
  $: filtered = searched.filter(shows[filter]);
  $: narrowed = !!query || filter !== 'all';
  /** Excluded requirements need no plan, so only active ones are counted. */
  $: active = requirements.filter(r => r.active);
  $: covered = active.filter(r => coverageSummary(r).state === 'covered').length;
  $: sections = groupOrder(filtered).map(name => {
    const list = filtered.filter(r => groupName(r) === name);
    const inGate = list.filter(r => r.active);
    return { name, requirements: list, active: inGate.length, covered: inGate.filter(r => coverageSummary(r).state === 'covered').length, expanded: narrowed || !!open[name] };
  });
  $: pendingPlans = coverageProposals.filter(p => p.status === 'pending' && !accepted.includes(p.id));
  $: openSuggestions = suggestions.filter(s => s.status === 'open');
  /** Open suggestions for an existing requirement. A new requirement has none to mark. */
  $: changeSuggestions = openSuggestions.filter(s => s.requirementId);
  /** Requirements with a proposal to approve, in sidebar order. */
  $: reviewQueue = requirements.filter(r => pendingPlans.some(p => p.requirementId === r.id)).map(r => r.id);
  $: reviewable = !coverageEditing && requirement ? pendingPlans.find(p => p.requirementId === requirement.id) : undefined;
  /** Requirements whose proposal is approved into this draft. */
  $: approvals = coverageProposals.filter(p => accepted.includes(p.id));
  $: approvedIds = approvals.map(p => p.requirementId);
  $: staged = suggestions.filter(s => stagedSuggestions.includes(s.id));
  $: changes = draftChanges(requirements, revision.requirements, approvals, staged);
  /** Selects the next (or previous) requirement with a proposal to review, wrapping around. */
  function stepReview(delta: 1 | -1) {
    if (!reviewQueue.length) return;
    const order = requirements.map(r => r.id), at = order.indexOf(selected);
    const after = reviewQueue.filter(id => order.indexOf(id) > at), before = reviewQueue.filter(id => order.indexOf(id) < at);
    const id = delta > 0 ? after[0] ?? reviewQueue[0] : before[before.length - 1] ?? reviewQueue[reviewQueue.length - 1];
    show(id);
  }
  /** After a decision, moves on unless this requirement still has a proposal. */
  async function advance() {
    await tick();
    if (!reviewQueue.includes(selected)) stepReview(1);
  }
  /** Selects a requirement, clearing a search or filter that would hide it. */
  export function show(id: string, next: Filter = filter) {
    const target = requirements.find(r => r.id === id);
    if (target && !matches(target, query)) query = '';
    if (target && !shows[next](target)) next = 'all';
    filter = next;
    if (target) select(target.id); else narrow(query, next);
  }
  function fail(e: unknown) { error = message(e); errorAt = e instanceof ApiError ? e.at : undefined; }
  $: if (!error) errorAt = undefined;
  /** Opens the requirement the failure names, and its plan when the failure is in one. */
  function showProblem() {
    const at = errorAt;
    const target = at?.requirementId ? requirements.find(r => r.id === at.requirementId) : undefined;
    if (!at || !target) return;
    show(target.id);
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
  /** Applies a search and a filter, and selects the first match when they hide the selection. */
  function narrow(term: string, next: Filter) {
    const list = requirements.filter(r => matches(r, term) && shows[next](r));
    if (selected && !list.some(r => r.id === selected)) { leaveEditor(); selected = list[0]?.id || ''; }
    query = term; filter = next;
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
  async function startEdit() {
    deleting = false; edit = true;
    await tick(); titleField?.focus();
  }
  async function create() {
    if (busy) return;
    leaveEditor();
    busy = true; error = '';
    try {
      const { id } = await api<{ id: string }>('/api/requirements/next-id', 'POST');
      const group = groupName(requirement);
      const r: Requirement = { ...(group ? { group } : {}), id, title: '', statement: '', active: true, tests: [] };
      const at = requirement ? requirements.indexOf(requirement) + 1 : requirements.length;
      requirements = [...requirements.slice(0, at), r, ...requirements.slice(at)]; selected = r.id; query = ''; filter = 'all'; edit = true; reveal(r.id);
      await tick(); document.getElementById('requirement-title')?.focus();
    } catch (e) { fail(e); } finally { busy = false; }
  }
  /** Ctrl/Cmd+Enter in the requirement editor finishes this requirement and starts the next one in the same group. */
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
    const gone = requirement.id;
    requirements = requirements.filter(r => r.id !== gone);
    stagedSuggestions = stagedSuggestions.filter(id => suggestions.find(s => s.id === id)?.requirementId !== gone);
    selected = filtered.find(r => r.id !== gone)?.id || '';
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
    if (incomplete) { error = `${incomplete.id} needs a title and statement before you save.`; show(incomplete.id); return; }
    for (const r of requirements) if (r.coverage && planBlank(r.coverage)) delete r.coverage;
    const invalid = requirements.find(r => r.coverage && planProblem(r.coverage));
    // Opening the editor clears the message, so it is set after.
    if (invalid) { show(invalid.id); editCoverage(); error = `${invalid.id} coverage: ${planProblem(invalid.coverage!)}`; return; }
    busy = true;
    try {
      const saved = await api<Revision>('/api/requirements', 'PUT', { baseRevision: revision.id, requirements, ...(accepted.length ? { accept: accepted } : {}), ...(stagedSuggestions.length ? { acceptSuggestions: stagedSuggestions } : {}) });
      const written = changes.length;
      revision = saved; requirements = clone(saved.requirements); accepted = []; stagedSuggestions = []; deleted = []; deleting = false; edit = false; onsaved(saved);
      notice = `Revision r${saved.id} saved with ${written} ${written === 1 ? 'change' : 'changes'}. Existing candidates keep their original revision.`;
      await loadProposals(); await loadSuggestions();
    } catch (e) { fail(e); } finally { busy = false; }
  }
  async function refresh() {
    if (dirty && !confirm('Discard this draft and load the latest saved revision? Approved proposals and accepted suggestions stay open for review.')) return;
    busy = true; error = ''; notice = '';
    try { const versions = await api<Revision[]>('/api/revisions'); const latest = versions.sort((a,b) => b.id - a.id)[0]; if (latest) { revision = latest; requirements = clone(latest.requirements); query = ''; if (!requirements.some(r => r.id === selected)) selected = requirements[0]?.id || ''; coverageEditing = false; edit = false; accepted = []; stagedSuggestions = []; deleted = []; deleting = false; onsaved(latest); reveal(selected); } await loadProposals(); await loadSuggestions(); }
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
  async function loadSuggestions() {
    suggestions = await api<RequirementSuggestionReview[]>('/api/requirement-suggestions');
    // The save refuses a staged id that is no longer open.
    stagedSuggestions = stagedSuggestions.filter(id => suggestions.some(s => s.id === id && s.status === 'open'));
  }
  onMount(() => { if (selected) reveal(selected); Promise.all([loadProposals(), loadSuggestions()]).catch(e => { error = message(e); }); });
  /** Declines or reopens at once. Accepting is staged and recorded with the next revision. */
  async function decideSuggestion(id: string, answer: false | 'reopen', feedback = '') {
    if (busy) return false;
    busy = true; error = '';
    const body = answer === 'reopen' ? { reopen: true } : { accept: answer, ...(feedback ? { feedback } : {}) };
    try { await api(`/api/requirement-suggestions/${id}`, 'POST', body); return true; }
    catch (e) { fail(e); return false; }
    finally { busy = false; await loadSuggestions().catch(() => { /* Keep the decision error. */ }); }
  }
  /**
   * Approving applies the proposal's plan and procedures to the draft. Nothing is recorded until the
   * revision is saved, so a round of reviews is one revision instead of one per proposal.
   */
  function approve(proposal: CoverageProposalReview) {
    if (busy || accepted.includes(proposal.id)) return;
    const target = requirements.find(r => r.id === proposal.requirementId);
    if (!target) { error = `This draft has no requirement ${proposal.requirementId}.`; return; }
    error = '';
    const next: Requirement = { ...target, tests: clone(target.tests), coverage: clone(proposal.plan) };
    linkEvidence(next, next.coverage!, catalog, () => crypto.randomUUID(), clone(proposal.procedures ?? []));
    update(next);
    accepted = [...accepted, proposal.id];
    notice = `${proposal.requirementId} approved into your draft. Save the revision to record it.`;
    advance();
  }
  /** Takes one approval back out of the draft, with the plan and tests it brought. */
  function unapprove(proposalId: string) {
    const proposal = coverageProposals.find(p => p.id === proposalId);
    const saved = revision.requirements.find(r => r.id === proposal?.requirementId);
    const target = requirements.find(r => r.id === proposal?.requirementId);
    if (busy || !proposal || !saved || !target) return;
    if (selected === target.id) leaveEditor();
    update({ ...target, tests: clone(saved.tests), coverage: saved.coverage && clone(saved.coverage) });
    accepted = accepted.filter(id => id !== proposalId);
    notice = '';
  }
  async function reject(id: string, feedback = '') {
    if (busy) return;
    busy = true; error = '';
    try {
      await api(`/api/coverage-proposals/${id}`, 'POST', { accept: false, feedback });
      rejecting = false;
      await loadProposals();
      notice = 'Proposal rejected. The agent can read your feedback.';
      advance();
    } catch (e) {
      fail(e);
      try { await loadProposals(); } catch { /* Keep the original decision error. */ }
    } finally { busy = false; }
  }
  /** Review keys. Letters act only outside text fields and the suggestions panel, which has its own. */
  function reviewKeys(event: KeyboardEvent) {
    const target = event.target instanceof Element ? event.target : null;
    if (!visible) return;
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') { event.preventDefault(); if (dirty && !busy && !procedureOpen) save(); return; }
    // The panel closes its own feedback box, so Escape there leaves the coverage review alone.
    if (target?.closest('.suggestions')) return;
    if (event.key === 'Escape' && rejecting) { rejecting = false; return; }
    if (event.metaKey || event.ctrlKey || event.altKey || target?.closest('input, textarea, select, [contenteditable]')) return;
    const key = event.key.toLowerCase();
    if (key === 'j' || key === 'k') { event.preventDefault(); stepReview(key === 'j' ? 1 : -1); }
    else if (!reviewable || busy || rejecting) return;
    else if (key === 'a' && !reviewable.conflict) { event.preventDefault(); approve(reviewable); }
    else if (key === 'r') { event.preventDefault(); rejecting = true; }
  }
  const same = (a: unknown, b: unknown) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null);
  /** What the next save writes: one line per requirement and per staged decision. */
  function draftChanges(draft: Requirement[], saved: Requirement[], approvals: CoverageProposalReview[], staged: RequirementSuggestionReview[]) {
    const lines: { key: string; label: string; undo?: () => void }[] = [];
    for (const r of draft) {
      const old = saved.find(s => s.id === r.id);
      if (!old) { lines.push({ key: r.id, label: `${r.id} · new` }); continue; }
      const approval = approvals.find(p => p.requirementId === r.id);
      const parts = [
        ...(approval ? ['plan approved'] : !same(old.coverage, r.coverage) || !same(old.tests, r.tests) ? ['coverage edited'] : []),
        ...(old.title !== r.title ? ['title edited'] : []),
        ...(old.statement !== r.statement ? ['statement edited'] : []),
        ...(groupName(old) !== groupName(r) ? ['group changed'] : []),
        ...(!!old.todo !== !!r.todo || !!old.implementationNeeded !== !!r.implementationNeeded || old.active !== r.active ? ['labels changed'] : [])
      ];
      if (parts.length) lines.push({ key: r.id, label: `${r.id} · ${parts.join(', ')}`, undo: approval && (() => unapprove(approval.id)) });
    }
    for (const r of saved) if (!draft.some(d => d.id === r.id)) lines.push({ key: r.id, label: `${r.id} · deleted` });
    const kept = saved.map(r => r.id).filter(id => draft.some(d => d.id === id));
    if (!same(kept, draft.map(r => r.id).filter(id => kept.includes(id)))) lines.push({ key: 'order', label: 'Order changed' });
    for (const s of staged) lines.push({ key: s.id, label: `Suggestion “${s.title}” · accepted`, undo: () => { stagedSuggestions = stagedSuggestions.filter(id => id !== s.id); } });
    return lines;
  }
</script>
<svelte:window on:keydown={reviewKeys} />
<div class="page-heading row"><div><div class="eyebrow">Product verification</div><h1>Requirements</h1><p class="muted">What the product must do, and the tests that show it does.</p></div><div class="actions"><details class="menu"><summary class="button">More ▾</summary><div class="menu-list"><label class="menu-item">Import Markdown…<input class="visually-hidden" type="file" accept=".md,.markdown,text/markdown,text/plain" disabled={busy} on:change={importMarkdown} /></label><button class="menu-item" on:click={exportMarkdown}>Export Markdown</button><button class="menu-item" on:click={() => manageGroups = !manageGroups}>Manage groups</button><button class="menu-item" on:click={showHistory}>Revision history</button></div></details>{#if openSuggestions.length || suggestionsOpen}<button class="suggest-toggle" aria-pressed={suggestionsOpen} disabled={busy} title="What agents suggest; you write the requirement" on:click={() => suggestionsOpen = !suggestionsOpen}><span class="review-count">{openSuggestions.length}</span>Suggestions</button>{/if}<button class="primary" disabled={busy} on:click={create}>+ Requirement</button></div></div>
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
<div class="workbench" class:with-suggestions={suggestionsOpen}><aside><div class="sidebar-scroll">
  <label class="search-label">Find a requirement<input type="search" value={query} on:input={(event) => narrow(event.currentTarget.value, filter)} placeholder="ID, title, text, or group…" /></label>
  <div class="chips" role="group" aria-label="Filter requirements">{#each Object.entries(FILTERS) as [key, label] (key)}{@const f = key as Filter}<button type="button" class="chip" aria-pressed={filter === f} on:click={() => narrow(query, f)}>{label} <span class="chip-count">{searched.filter(shows[f]).length}</span></button>{/each}</div>
  <div class="muted small sidebar-caption">{narrowed ? `${filtered.length} of ${requirements.length} match` : `${covered} of ${active.length} covered${reviewQueue.length ? ` · ${reviewQueue.length} to review` : ''}`} · r{revision.id}</div>
  {#each sections as section (section.name)}
    <button type="button" class="group-heading" aria-expanded={section.expanded} on:click={() => open = { ...open, [section.name]: !section.expanded }}><span class="chevron" aria-hidden="true">{section.expanded ? '▾' : '▸'}</span><span class="group-name">{section.name || 'Ungrouped'}</span>{#if section.active}<span class="group-count">{section.covered} of {section.active} covered</span>{/if}</button>
    {#if section.expanded}{#each section.requirements as r (r.id)}<button id={'entry-' + r.id} class="entry" class:selected={selected === r.id} on:click={() => select(r.id)}><span class="eyebrow">{r.id}</span><strong>{r.title || 'Untitled requirement'}</strong><span class="entry-status"><CoverageBadge requirement={r} approved={approvedIds.includes(r.id)} />{#if reviewQueue.includes(r.id)}<span class="entry-flag proposal">proposal</span>{/if}{#if changeSuggestions.some(s => s.requirementId === r.id)}<span class="entry-flag suggestion">suggestion</span>{/if}</span><RequirementLabels requirement={r} /></button>{/each}{/if}
  {:else}<p class="muted small">{requirements.length ? 'No matching requirements.' : 'Start with one important product promise, or import a Markdown draft.'}</p>{/each}
</div></aside>
<section class="detail">
{#if requirement}
  <div class="row"><span class="eyebrow">{requirement.id} · {groupName(requirement) || 'Ungrouped'}</span><div class="actions"><RequirementLabels {requirement} />{#if !edit}<button disabled={busy} on:click={startEdit}>Edit requirement</button>{/if}</div></div>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  {#if edit}<div class="section" on:keydown={editorKeys}><label>Title<input id="requirement-title" bind:this={titleField} bind:value={requirement.title} on:input={() => requirements = [...requirements]} placeholder="A clear product promise" /></label><ProseCheck text={requirement.title} language="plaintext" field={titleField} onfix={setTitle} /><GroupPicker label="Group" hint="Optional" value={requirement.group || ''} {groups} disabled={busy} onchange={setGroup} /><MarkdownField label="Requirement" bind:value={requirement.statement} on:input={() => requirements = [...requirements]} /><div class="section"><h3>Labels</h3><RequirementLabels {requirement} editable disabled={busy} onchange={update} /><p class="small muted">Click a label to apply or remove it. An incomplete definition blocks publication and cannot be excepted. Implementation needed blocks publication unless an administrator accepts a candidate exception. Excluded requirements stay visible outside release verification. Existing candidates never change.</p></div><div class="row"><div class="actions"><button on:click={() => edit = false}>Done editing</button><button class="primary" disabled={busy} on:click={create}>Done, add next <kbd>{mod}↵</kbd></button></div><div class="actions"><span class="small muted">Position in group</span><button class="text-button" disabled={busy} on:click={() => move(-1)}>↑ Move up</button><button class="text-button" disabled={busy} on:click={() => move(1)}>↓ Move down</button><button class="text-button danger" disabled={busy} on:click={() => deleting = !deleting}>Delete requirement</button></div></div>
    {#if deleting}<div class="alert warning"><strong>Delete {requirement.id} · {requirement.title || 'Untitled requirement'}?</strong><p>This removes the requirement and its {requirement.tests.length} tests from the draft. Save revision to apply. Existing revisions and release candidates stay unchanged.</p><div class="actions"><button class="danger" disabled={busy} on:click={removeRequirement}>Delete from draft</button><button on:click={() => deleting = false}>Keep requirement</button></div></div>{/if}
  </div>
  {:else}<h2 class="requirement-title">{requirement.title || 'Untitled requirement'}</h2><div class="requirement-statement"><Markdown text={requirement.statement} /></div>
    {@const suggested = changeSuggestions.filter(s => s.requirementId === requirement.id)}
    {#if suggested.length}<div class="on-req">{suggested.length} suggested {suggested.length === 1 ? 'change' : 'changes'} · <button on:click={() => { suggestionsOpen = true; expandedSuggestion = suggested[0].id; }}>Show in Suggestions</button></div>{/if}{/if}
  {@const saved = revision.requirements.find(r => r.id === requirement.id)}
  {@const changed = !saved || coverageDefinition(saved) !== coverageDefinition(requirement)}
  {@const plans = pendingPlans.filter(p => p.requirementId === requirement.id)}
  {@const decided = coverageProposals.filter(p => p.requirementId === requirement.id && p.status !== 'pending' && p.status !== 'superseded')}
  <section class="section coverage-section" aria-label="Requirement coverage">
    <div class="row"><div class="row coverage-head"><h2>Coverage</h2><CoverageBadge {requirement} approved={approvedIds.includes(requirement.id)} /></div>{#if !coverageEditing}<button disabled={busy} on:click={editCoverage}>{requirement.coverage ? 'Edit coverage' : 'Define coverage'}</button>{/if}</div>
    {#if coverageEditing}<CoverageEditor {requirement} {catalog} {busy} flag={flagged} bind:editing={procedureOpen} onchange={() => requirements = [...requirements]} ondone={closeCoverage} onrefresh={refreshCatalog} />
    {:else}
      {#each plans as p (p.id)}<CoverageProposal proposal={p} {requirement} {catalog} {busy} bind:rejecting onapprove={approve} onreject={reject} />{/each}
      {#if requirement.coverage}
        {#if plans.length}<p class="eyebrow current-plan">Current plan</p>{/if}
        {#if changed}<p class="small muted">Approved when you save the revision.</p>
        {:else if requirement.coverage.review}<p class="small muted">Approved by {requirement.coverage.review.author} · {date(requirement.coverage.review.createdAt)}{#if requirement.coverage.review.sourceSha}{' · '}catalogue <code>{requirement.coverage.review.sourceSha.slice(0, 10)}</code>{/if}</p>{/if}
        <CoveragePlan plan={requirement.coverage} {requirement} {catalog} />
      {:else if !plans.length}<div class="empty coverage-empty"><h3>What would prove this requirement?</h3><p class="muted">Break it into checkable criteria, attach the tests that prove each one, and note the gaps. Define coverage to start, or wait for an agent proposal.</p></div>{/if}
      {#if decided.length}<details class="small history"><summary>Earlier proposals ({decided.length})</summary>{#each decided as p (p.id)}<p class="small wrap"><span class="badge" class:success={p.status === 'accepted'} class:error={p.status === 'rejected'}>{p.status}</span> {p.author} · {date(p.createdAt)}{#if p.decidedBy} · decided by {p.decidedBy}{/if}{#if p.feedback} · “{p.feedback}”{/if}</p>{/each}</details>{/if}
    {/if}
  </section>
{:else}<div class="empty"><h2>{requirements.length ? 'No matching requirements' : 'Make the important promises explicit.'}</h2><p class="muted">{requirements.length ? 'Clear the search, or choose another filter.' : 'Write a measurable requirement, then define the checks that provide evidence. You can also import a Markdown draft.'}</p>{#if requirements.length}<button on:click={() => narrow('', 'all')}>Show all requirements</button>{:else}<button class="primary" disabled={busy} on:click={create}>Create requirement</button>{/if}</div>{/if}
</section>
{#if suggestionsOpen}<Suggestions {suggestions} {requirements} revisionId={revision.id} {busy} bind:expanded={expandedSuggestion} bind:staged={stagedSuggestions} onselect={show} onclose={() => suggestionsOpen = false} ondecide={decideSuggestion} />{/if}</div>
{#if dirty || error || notice || reviewQueue.length}<div class="savebar" role="region" aria-label="Draft and review">
  {#if error}<div class="alert error" role="alert"><span class="grow">{error}</span>{#if errorAt?.requirementId && requirements.some(r => r.id === errorAt?.requirementId)}<button class="text-button" disabled={busy} on:click={showProblem}>Show {errorAt.requirementId}{errorAt.criterion ? ` · criterion ${errorAt.criterion}` : ''}</button>{/if}<button class="text-button" disabled={busy} on:click={refresh}>Reload saved revision</button></div>{/if}
  {#if notice}<p class="small success notice" role="status">{notice}</p>{/if}
  <div class="row">
    {#if reviewQueue.length}{@const at = reviewQueue.indexOf(selected)}<div class="queue actions"><button class="text-button" disabled={busy} aria-label="Previous proposal" on:click={() => stepReview(-1)}>‹ <kbd>K</kbd></button><span class="small"><strong>{at < 0 ? `${reviewQueue.length} ${reviewQueue.length === 1 ? 'proposal' : 'proposals'} to review` : `Proposal ${at + 1} of ${reviewQueue.length}`}</strong></span><button class="text-button" disabled={busy} aria-label="Next proposal" on:click={() => stepReview(1)}><kbd>J</kbd> ›</button></div>{/if}
    <span class="small grow">{#if dirty}<strong>Unsaved draft · {changes.length} {changes.length === 1 ? 'change' : 'changes'}</strong> <span class="muted">· Candidates use saved revisions only.</span>{:else}<span class="muted">Saved revision r{revision.id}</span>{/if}</span>
    {#if dirty}<div class="actions"><button disabled={busy} on:click={refresh}>Discard draft</button><button class="primary" disabled={busy || procedureOpen} title={procedureOpen ? 'Keep or cancel the manual procedure first.' : undefined} on:click={save}>{busy ? 'Saving…' : 'Save revision'} <kbd>{mod}S</kbd></button></div>{/if}
  </div>
  {#if dirty && changes.length}<ul class="changes small">{#each changes as c (c.key)}<li><span>{c.label}</span>{#if c.undo}<button class="text-button small" disabled={busy} on:click={c.undo}>Undo</button>{/if}</li>{/each}</ul>{/if}
</div>{/if}
<style>
  .alert .grow, .savebar .grow { flex: 1; min-width: 200px; }
  .savebar .alert { margin: 0 0 10px; }
  .notice { margin: 0 0 8px; }
  .queue { gap: 4px; }
  .changes { list-style: none; margin: 10px 0 0; padding: 8px 0 0; border-top: 1px solid var(--line); max-height: 8.5em; overflow: auto; }
  .changes li { display: flex; gap: 10px; align-items: center; min-height: 26px; }
  .changes .text-button { padding: 0; }
  .chips { display: flex; flex-wrap: wrap; gap: 5px; margin: -6px 0 10px; }
  .chip { padding: 3px 9px; font-size: 11px; border-radius: 999px; }
  .chip[aria-pressed='true'] { background: var(--soft); border-color: var(--forest); color: var(--forest); font-weight: 600; }
  .chip-count { opacity: .7; }
  kbd { font: 600 10px/1 ui-monospace, SFMono-Regular, Consolas, monospace; padding: 2px 4px; border: 1px solid currentColor; border-radius: 3px; opacity: .7; }
  .workbench.with-suggestions { grid-template-columns: 240px minmax(0, 1fr) 360px; }
  .on-req { display: inline-flex; align-items: center; gap: 8px; margin-top: 14px; padding: 6px 10px; font-size: 12px; color: var(--slate-strong); background: var(--slate-bg); border: 1px solid var(--slate-line); border-radius: 6px; }
  .on-req button { padding: 2px 0; font-size: 12px; font-weight: 600; color: var(--slate); background: transparent; border-color: transparent; text-decoration: underline; text-underline-offset: 3px; }
  @media (max-width: 1100px) { .workbench.with-suggestions { grid-template-columns: 210px minmax(0, 1fr) 320px; } }
  @media (max-width: 900px) { .workbench.with-suggestions { grid-template-columns: 210px minmax(0, 1fr); } }
  @media (max-width: 650px) { .workbench.with-suggestions { grid-template-columns: 1fr; } }
</style>
