<script lang="ts">
  import { onMount, tick } from 'svelte';
  import type { Revision, Requirement, Catalog, VerificationTest, ProposalReview, CoverageProposalReview } from '$lib/types';
  import { api, clone, date, message } from './api';
  import { parseRequirements, formatRequirements } from './markdown-requirements';
  import Markdown from './Markdown.svelte';
  import MarkdownField from './MarkdownField.svelte';
  import TestEditor from './TestEditor.svelte';
  import Files from './Files.svelte';
  import RequirementGroups from './RequirementGroups.svelte';
  import RequirementLabels from './RequirementLabels.svelte';
  import LinkSuggestions from './LinkSuggestions.svelte';
  import CoveragePlan from './CoveragePlan.svelte';
  import CoverageEditor from './CoverageEditor.svelte';
  import CoverageBadge from './CoverageBadge.svelte';
  import CoverageProposal from './CoverageProposal.svelte';
  import { absorbedBy, coverageDefinition, coverageIssues, mappedBy, planBlank, planProblem } from '$lib/coverage';
  export let revision: Revision;
  export let catalog: Catalog;
  export let dirty = false;
  export let onsaved: (revision: Revision) => void;
  export let oncatalog: (catalog: Catalog) => void;
  let requirements: Requirement[] = clone(revision.requirements);
  let selected = requirements[0]?.id || '';
  let edit = false;
  let testDraft: VerificationTest | null = null;
  let picker = false;
  let coverageEditing = false;
  let search = '';
  let query = '';
  let open: Record<string, boolean> = { [groupName(requirements[0])]: true };
  let manageGroups = false;
  let busy = false;
  let error = '';
  let notice = '';
  let history: Revision[] | null = null;
  let historical: Revision | null = null;
  let proposals: ProposalReview[] = [];
  let coverageProposals: CoverageProposalReview[] = [];
  let deleting = false;
  let deleted: { requirement: Requirement; index: number }[] = [];
  let undo: { requirementId: string; test: VerificationTest; index: number } | null = null;
  $: testChanged = testDraft !== null && JSON.stringify(testDraft) !== JSON.stringify(requirement?.tests.find(t => t.id === testDraft?.id));
  $: dirty = (requirement, JSON.stringify(requirements) !== JSON.stringify(revision.requirements) || testChanged);
  let approveSha = '';
  $: approveSha = catalog.sourceSha;
  $: requirement = requirements.find(r => r.id === selected);
  $: groups = groupOrder(requirements).filter(Boolean).map(name => ({ name, count: requirements.filter(r => groupName(r) === name).length }));
  $: filtered = requirements.filter(r => matches(r, query));
  $: sections = groupOrder(filtered).map(name => { const list = filtered.filter(r => groupName(r) === name); return { name, requirements: list, unverified: list.filter(r => r.active && coverageIssues(r).length > 0).length, expanded: !!query || !!open[name] }; });
  $: matching = catalog.cases.filter(c => `${c.name} ${c.suite} ${c.id} ${c.file || ''}`.toLowerCase().includes(search.toLowerCase()));
  $: pendingPlans = coverageProposals.filter(p => p.status === 'pending');
  $: pendingLinks = proposals.filter(p => p.status === 'pending');
  /** Requirements with something to approve, in sidebar order. */
  $: reviewQueue = requirements.filter(r => pendingPlans.some(p => p.requirementId === r.id) || pendingLinks.some(p => p.requirementId === r.id)).map(r => r.id);
  function nextReview() {
    const after = reviewQueue.indexOf(selected);
    const id = reviewQueue[(after + 1) % reviewQueue.length];
    if (query && !matches(requirements.find(r => r.id === id)!, query)) query = '';
    select(id);
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
    if (selected && !next.some(r => r.id === selected)) {
      if (!leaveEditor()) return false;
      selected = next[0]?.id || '';
    }
    query = term; return true;
  }
  function setGroup(value: string) { if (!busy && requirement) update({ ...requirement, group: value }); }
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
    if (testChanged && !confirm('Discard the unsaved manual test changes?')) return false;
    testDraft = null; edit = false; picker = false; deleting = false; return true;
  }
  function select(id: string) { if (leaveEditor()) { selected = id; reveal(id); } }
  async function create() {
    if (busy || !leaveEditor()) return;
    busy = true; error = '';
    try {
      const { id } = await api<{ id: string }>('/api/requirements/next-id', 'POST');
      if (!leaveEditor()) return;
      const group = groupName(requirement);
      const r: Requirement = { ...(group ? { group } : {}), id, title: '', statement: '', active: true, tests: [] };
      const at = requirement ? requirements.indexOf(requirement) + 1 : requirements.length;
      requirements = [...requirements.slice(0, at), r, ...requirements.slice(at)]; selected = r.id; query = ''; edit = true; reveal(r.id);
      await tick(); document.getElementById('requirement-title')?.focus();
    } catch (e) { error = message(e); } finally { busy = false; }
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
    if (busy || !requirement || !leaveEditor()) return;
    deleted = [...deleted, { requirement: clone(requirement), index: requirements.indexOf(requirement) }];
    requirements = requirements.filter(r => r.id !== selected);
    selected = requirements.find(r => matches(r, query))?.id || '';
    deleting = false; edit = false; undo = null;
  }
  function restoreRequirement() {
    if (busy || !deleted.length || !leaveEditor()) return;
    const item = deleted[deleted.length - 1];
    const restored = [...requirements]; restored.splice(item.index, 0, item.requirement); requirements = restored;
    deleted = deleted.slice(0, -1); query = ''; selected = item.requirement.id; reveal(selected);
  }
  function update(r: Requirement) { requirements = requirements.map(value => value.id === r.id ? r : value); }
  function editCoverage() {
    if (!requirement) return;
    if (!requirement.coverage) requirement.coverage = { rationale: '', criteria: [{ id: crypto.randomUUID(), statement: '', evidence: [], gap: '' }] };
    coverageEditing = true; edit = false; manageGroups = false; picker = false; error = ''; notice = ''; requirements = [...requirements];
  }
  /** An untouched plan is dropped so the requirement stays "not assessed". */
  function closeCoverage() {
    if (coverageEditing && requirement?.coverage && planBlank(requirement.coverage)) delete requirement.coverage;
    coverageEditing = false; requirements = [...requirements];
  }
  function keepTest(test: VerificationTest) {
    if (!requirement) return;
    update({ ...requirement, tests: requirement.tests.some(t => t.id === test.id) ? requirement.tests.map(t => t.id === test.id ? test : t) : [...requirement.tests, test] });
    testDraft = null;
  }
  function removeTest(test: VerificationTest) {
    if (!requirement) return;
    undo = { requirementId: requirement.id, test: clone(test), index: requirement.tests.indexOf(test) };
    update({ ...requirement, tests: requirement.tests.filter(t => t.id !== test.id) });
  }
  function restoreTest() {
    if (!undo) return;
    const r = requirements.find(r => r.id === undo?.requirementId);
    if (r) { const tests = [...r.tests]; tests.splice(undo.index, 0, undo.test); update({ ...r, tests }); }
    undo = null;
  }
  function link(caseId: string, title: string) { if (requirement) update({ ...requirement, tests: [...requirement.tests, { id: crypto.randomUUID(), kind: 'automated', title, caseId, inputs: [] }] }); }
  async function importMarkdown(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0]; input.value = '';
    if (!file || !leaveEditor()) return;
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
    } catch (e) { error = message(e); } finally { busy = false; }
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
    if (testDraft) { error = 'Keep or cancel the manual test changes before saving the revision.'; return; }
    const incomplete = requirements.find(r => !r.title.trim() || !r.statement.trim());
    if (incomplete) { error = `${incomplete.id} needs a title and statement before you save.`; select(incomplete.id); return; }
    for (const r of requirements) if (r.coverage && planBlank(r.coverage)) delete r.coverage;
    const invalid = requirements.find(r => r.coverage && planProblem(r.coverage));
    if (invalid) { error = `${invalid.id} coverage: ${planProblem(invalid.coverage!)}`; select(invalid.id); editCoverage(); return; }
    busy = true;
    try {
      const saved = await api<Revision>('/api/requirements', 'PUT', { baseRevision: revision.id, requirements });
      revision = saved; requirements = clone(saved.requirements); undo = null; deleted = []; deleting = false; edit = false; onsaved(saved); notice = `Revision r${saved.id} saved. Existing candidates keep their original revision.`;
      await loadProposals();
    } catch (e) { error = message(e); } finally { busy = false; }
  }
  async function approveCoverage() {
    if (!requirement || busy) return;
    busy = true; error = ''; notice = '';
    try {
      const saved = await api<Revision>(`/api/requirements/${requirement.id}/coverage/approve`, 'POST', { baseRevision: revision.id, sourceSha: approveSha });
      revision = saved; requirements = clone(saved.requirements); onsaved(saved);
      notice = `Coverage approved for commit ${approveSha.slice(0, 10)}. Test results and existing candidates are unchanged.`;
      await loadProposals();
    } catch (e) { error = message(e); } finally { busy = false; }
  }
  async function refresh() {
    if (dirty && !confirm('Discard this draft and load the latest saved revision?')) return;
    busy = true; error = '';
    try { const versions = await api<Revision[]>('/api/revisions'); const latest = versions.sort((a,b) => b.id - a.id)[0]; if (latest) { revision = latest; requirements = clone(latest.requirements); query = ''; if (!requirements.some(r => r.id === selected)) selected = requirements[0]?.id || ''; testDraft = null; coverageEditing = false; edit = false; undo = null; deleted = []; deleting = false; onsaved(latest); reveal(selected); } await loadProposals(); }
    catch (e) { error = message(e); } finally { busy = false; }
  }
  async function refreshCatalog() {
    busy = true; error = '';
    try { catalog = await api<Catalog>('/api/catalog'); oncatalog(catalog); } catch (e) { error = message(e); } finally { busy = false; }
  }
  async function showHistory() { error = ''; try { history = await api<Revision[]>('/api/revisions'); } catch (e) { error = message(e); } }
  async function loadProposals() {
    [proposals, coverageProposals] = await Promise.all([api<ProposalReview[]>('/api/proposals'), api<CoverageProposalReview[]>('/api/coverage-proposals')]);
  }
  onMount(() => { loadProposals().catch(e => { error = message(e); }); });
  async function decide(kind: 'proposals' | 'coverage-proposals', id: string, accept: boolean, feedback = '') {
    if (busy) return;
    if (accept && dirty) { error = 'Save or discard your draft before approving a proposal.'; return; }
    busy = true; error = '';
    try {
      const decided = await api<{ revision: Revision }>(`/api/${kind}/${id}`, 'POST', { accept, feedback });
      if (accept) {
        revision = decided.revision; requirements = clone(revision.requirements); onsaved(revision);
        if (!requirements.some(r => r.id === selected)) selected = requirements[0]?.id || '';
        undo = null; deleted = []; testDraft = null; edit = false; deleting = false;
      }
      await loadProposals();
      notice = accept ? kind === 'coverage-proposals' ? 'Coverage approved. Existing candidates keep their original coverage review.' : 'Test link saved. Coverage needs a fresh review.' : 'Proposal rejected. The agent can read your feedback.';
    } catch (e) {
      error = message(e);
      try { await loadProposals(); } catch { /* Keep the original decision error. */ }
    } finally { busy = false; }
  }
</script>
<svelte:window on:keydown={(event) => { if (edit) editorKeys(event); }} />
<div class="page-heading row"><div><div class="eyebrow">Product verification</div><h1>Requirements</h1><p class="muted">The promises we make, and how we check them.</p></div><div class="actions"><details class="menu"><summary class="button">More ▾</summary><div class="menu-list"><label class="menu-item">Import Markdown…<input class="visually-hidden" type="file" accept=".md,.markdown,text/markdown,text/plain" disabled={busy} on:change={importMarkdown} /></label><button class="menu-item" on:click={exportMarkdown}>Export Markdown</button><button class="menu-item" on:click={() => manageGroups = !manageGroups}>Manage groups</button><button class="menu-item" on:click={showHistory}>Revision history</button></div></details>{#if reviewQueue.length}<button class="review-queue" disabled={busy} title="Go to the next requirement with a proposal to review" on:click={nextReview}><span class="review-count">{reviewQueue.length}</span>{reviewQueue.length === 1 ? 'proposal to review' : 'proposals to review'}</button>{/if}<button class="primary" disabled={busy} on:click={create}>+ Requirement</button></div></div>
{#if error}<div class="alert error" role="alert">{error}<button class="text-button" disabled={busy} on:click={refresh}>Reload saved revision</button></div>{/if}
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
  <label class="search-label">Find a requirement<input type="search" value={query} on:input={(event) => { if (!setQuery(event.currentTarget.value)) event.currentTarget.value = query; }} placeholder="ID, title, text, or group…" /></label>
  <div class="muted small sidebar-caption">{query ? `${filtered.length} of ${requirements.length} match` : `${requirements.length} requirements · ${groups.length} groups`} · r{revision.id}</div>
  {#each sections as section (section.name)}
    <button type="button" class="group-heading" aria-expanded={section.expanded} on:click={() => open = { ...open, [section.name]: !section.expanded }}><span class="chevron" aria-hidden="true">{section.expanded ? '▾' : '▸'}</span><span class="group-name">{section.name || 'Ungrouped'}</span>{#if section.unverified}<span class="group-warning" title="{section.unverified} without complete coverage review">{section.unverified} ⚠</span>{/if}<span class="group-count">{section.requirements.length}</span></button>
    {#if section.expanded}{#each section.requirements as r (r.id)}<button id={'entry-' + r.id} class="entry" class:selected={selected === r.id} on:click={() => select(r.id)}><span class="entry-head"><span class="eyebrow">{r.id}</span>{#if r.active && !r.tests.length}<span class="entry-flag warning" title="No verification defined">no checks</span>{:else if r.tests.length}<span class="entry-flag muted">{r.tests.length} {r.tests.length === 1 ? 'check' : 'checks'}</span>{/if}</span><strong>{r.title || 'Untitled requirement'}</strong><span class="entry-status"><CoverageBadge requirement={r} />{#if reviewQueue.includes(r.id)}<span class="entry-flag proposal">proposal</span>{/if}</span><RequirementLabels requirement={r} /></button>{/each}{/if}
  {:else}<p class="muted small">{requirements.length ? 'No matching requirements.' : 'Start with one important product promise, or import a Markdown draft.'}</p>{/each}
</div></aside>
<section class="detail">
{#if requirement}
  <div class="row"><span class="eyebrow">{requirement.id} · {groupName(requirement) || 'Ungrouped'}</span><div class="actions"><RequirementLabels {requirement} />{#if !edit && !testDraft && !picker}<button disabled={busy} on:click={() => { deleting = false; edit = true; }}>Edit requirement</button><button class="danger" disabled={busy} on:click={() => deleting = !deleting}>Delete requirement</button>{/if}</div></div>
  {#if deleting}<div class="alert warning"><strong>Delete {requirement.id} · {requirement.title || 'Untitled requirement'}?</strong><p>This removes the requirement and its {requirement.tests.length} linked tests from the draft. Save revision to apply. Existing revisions and release candidates stay unchanged.</p><div class="actions"><button class="danger" disabled={busy} on:click={removeRequirement}>Delete from draft</button><button on:click={() => deleting = false}>Keep requirement</button></div></div>{/if}
  {#if edit}<div class="section"><label>Title<input id="requirement-title" bind:value={requirement.title} on:input={() => requirements = [...requirements]} placeholder="A clear product promise" /></label><label>Group <span class="small muted">· Optional</span><input disabled={busy} list="requirement-group-names" maxlength={80} value={requirement.group || ''} on:input={(event) => setGroup(event.currentTarget.value)} placeholder="Choose an existing group or type a new name" /><datalist id="requirement-group-names">{#each groups as group}<option value={group.name}></option>{/each}</datalist></label><MarkdownField label="Requirement" bind:value={requirement.statement} /><div class="section"><h3>Labels</h3><RequirementLabels {requirement} editable disabled={busy} onchange={update} /><p class="small muted">Click a label to apply or remove it. An incomplete definition blocks publication and cannot be excepted. Implementation needed blocks publication unless an administrator accepts a candidate exception. Excluded requirements stay visible outside release verification. Existing candidates never change.</p></div><div class="row"><div class="actions"><button on:click={() => edit = false}>Done editing</button><button class="primary" disabled={busy} title="Ctrl+Enter or ⌘+Enter" on:click={create}>Done, add next</button></div><div class="actions"><span class="small muted">Position in group</span><button class="text-button" disabled={busy} on:click={() => move(-1)}>↑ Move up</button><button class="text-button" disabled={busy} on:click={() => move(1)}>↓ Move down</button></div></div></div>
  {:else}<h2 class="requirement-title">{requirement.title || 'Untitled requirement'}</h2><div class="statement"><Markdown text={requirement.statement} /></div>{/if}
  {@const saved = revision.requirements.find(r => r.id === requirement.id)}
    {@const changed = !!saved?.coverage?.review && coverageDefinition(saved) !== coverageDefinition(requirement)}
  {@const plans = pendingPlans.filter(p => p.requirementId === requirement.id)}
  {@const suggestions = pendingLinks.filter(p => p.requirementId === requirement.id)}
  {@const links = suggestions.filter(p => !plans.some(plan => absorbedBy(plan.plan, requirement, p)))}
  {@const decided = coverageProposals.filter(p => p.requirementId === requirement.id && p.status !== 'pending' && p.status !== 'superseded')}
  <section class="section" aria-label="Requirement coverage">
    <div class="row"><div class="row coverage-head"><h2>Coverage</h2><CoverageBadge {requirement} {changed} /></div>{#if !coverageEditing}<button disabled={busy} on:click={editCoverage}>{requirement.coverage ? 'Edit coverage' : 'Define coverage'}</button>{/if}</div>
    {#if coverageEditing}<CoverageEditor {requirement} {catalog} {busy} onchange={() => requirements = [...requirements]} ondone={closeCoverage} />
    {:else}
      {#each plans as p (p.id)}<CoverageProposal proposal={p} {requirement} {catalog} {busy} {dirty} ondecide={(id, accept, feedback) => decide('coverage-proposals', id, accept, feedback)} />{/each}
      {#if suggestions.length > links.length}<p class="small muted absorbed">{suggestions.length - links.length} earlier link {suggestions.length - links.length === 1 ? 'suggestion is' : 'suggestions are'} part of the proposal above and resolve with it.</p>{/if}
      <LinkSuggestions suggestions={links} {busy} {dirty} ondecide={(id, accept) => decide('proposals', id, accept)} />
      {#if requirement.coverage}
        {#if plans.length}<p class="eyebrow current-plan">Current plan</p>{/if}
        {#if requirement.coverage.review && !changed}<p class="small muted">Approved by {requirement.coverage.review.author} · {date(requirement.coverage.review.createdAt)} · commit <code>{requirement.coverage.review.sourceSha.slice(0, 10)}</code></p>
        {:else if dirty}<p class="small muted">Save the revision, then approve the coverage.</p>
        {:else}<div class="approve row"><div class="actions"><button class="primary" disabled={busy || !/^[a-f0-9]{40}$/.test(approveSha)} on:click={approveCoverage}>Approve coverage</button><span class="small muted">{approveSha ? `for commit ${approveSha.slice(0, 10)}` : 'No CI catalogue commit yet'}</span></div><details class="small"><summary>Other commit</summary><input class="commit" maxlength={40} aria-label="Assessed commit" bind:value={approveSha} placeholder="Full 40-character commit" /></details></div>{/if}
        <CoveragePlan plan={requirement.coverage} {requirement} {catalog} />
      {:else if !plans.length}<div class="empty coverage-empty"><h3>What would prove this requirement?</h3><p class="muted">Break it into checkable criteria, attach the tests that prove each one, and note the gaps. Define coverage to start, or wait for an agent proposal.</p></div>{/if}
      {#if decided.length}<details class="small history"><summary>Earlier proposals ({decided.length})</summary>{#each decided as p (p.id)}<p class="small wrap"><span class="badge" class:success={p.status === 'accepted'} class:error={p.status === 'rejected'}>{p.status}</span> {p.author} · {date(p.createdAt)}{#if p.decidedBy} · decided by {p.decidedBy}{/if}{#if p.feedback} · “{p.feedback}”{/if}</p>{/each}</details>{/if}
      <details class="linked-tests"><summary>Linked tests and manual procedures ({requirement.tests.length})</summary>
  {#if testDraft}<div class="section"><TestEditor bind:test={testDraft} onsave={keepTest} oncancel={() => { if (!testChanged || confirm('Discard these manual test changes?')) testDraft = null; }} /></div>
  {:else if picker}<div class="section"><div class="row"><h2>Link an automated test</h2><div class="actions"><button disabled={busy} on:click={refreshCatalog}>Refresh catalogue</button><button on:click={() => picker = false}>Back to requirement</button></div></div><p class="muted small">Select a real test from the latest CI catalogue. CI still runs complete suites.</p><input type="search" aria-label="Search automated tests" bind:value={search} placeholder="Search test name, suite, or file…" />{#if catalog.sourceSha}<p class="muted small">Catalogue from <code>{catalog.sourceSha.slice(0, 10)}</code> · {date(catalog.updatedAt)}</p>{/if}<div class="catalog">{#if matching.length > 100}<p class="small muted">Showing the first 100 of {matching.length} matches. Refine your search to find a specific test.</p>{/if}{#each matching.slice(0, 100) as c}<div class="test"><div class="row"><h3>{c.name}</h3><button disabled={requirement.tests.some(t => t.caseId === c.id)} on:click={() => link(c.id, c.name)}>{requirement.tests.some(t => t.caseId === c.id) ? 'Linked' : 'Link test'}</button></div><div class="small muted">{c.suite}{c.file ? ' · ' + c.file : ''}</div><code class="wrap small">{c.id}</code></div>{:else}<div class="empty"><h3>{catalog.cases.length ? 'No matching tests' : 'No CI catalogue yet'}</h3><p class="muted">{catalog.cases.length ? 'Try a test name or suite name.' : 'The verification workflow imports real test results. After its first run, refresh the catalogue to browse available tests.'}</p></div>{/each}</div></div>
  {:else}<div class="section"><div class="row"><h2>Linked tests <span class="muted">{requirement.tests.length}</span></h2><div class="actions"><button on:click={() => picker = true}>Link automated test</button><button on:click={() => testDraft = { id: crypto.randomUUID(), kind: 'manual', title: '', steps: '', expected: '', inputs: [] }}>+ Manual test</button></div></div>
    {#if undo}<div class="alert" role="status">Test removed from draft. <button class="text-button" on:click={restoreTest}>Undo</button></div>{/if}
    {#each requirement.tests as t}<div class="test"><div class="row"><div><div class="eyebrow">{t.kind}</div><h3>{t.title}</h3></div><div class="actions">{#if t.kind === 'manual'}<button on:click={() => testDraft = clone(t)}>Edit test</button>{/if}{#if requirement.coverage && mappedBy(requirement.coverage, t)}<span class="small muted" title="Remove it from its criterion first">Used as evidence</span>{:else}<button class="text-button danger" on:click={() => removeTest(t)}>{t.kind === 'manual' ? 'Remove test' : 'Unlink'}</button>{/if}</div></div>{#if t.kind === 'automated'}<code class="wrap small">{t.caseId}</code>{#if !catalog.cases.some(c => c.id === t.caseId)}<p class="warning small">This test is absent from the latest catalogue. Check the link before preparing a release.</p>{/if}{:else}<details><summary>Procedure and input files</summary><Markdown text={t.steps || ''} /><h4>Expected result</h4><Markdown text={t.expected || ''} /><Files files={t.inputs} label="Input files" /></details>{/if}</div>{:else}<div class="empty"><h3>How will we verify this?</h3><p class="muted">Link an automated test or add a manual procedure. An active requirement without checks blocks publication.</p></div>{/each}</div>{/if}
      </details>
    {/if}
  </section>
{:else}<div class="empty"><h2>{requirements.length ? 'No matching requirements' : 'Make the important promises explicit.'}</h2><p class="muted">{requirements.length ? 'Clear your search or choose another group.' : 'Write a measurable requirement, then define the checks that provide evidence. You can also import a Markdown draft.'}</p><button class="primary" disabled={busy} on:click={create}>Create requirement</button></div>{/if}
</section></div>
{#if dirty}<div class="savebar row"><span class="small">{dirty ? 'You have unsaved changes' : `Saved revision r${revision.id}`} <span class="muted">· Candidates use saved revisions only.</span></span><div class="actions">{#if dirty}<button disabled={busy} on:click={refresh}>Discard draft</button>{/if}<button class="primary" disabled={busy || !dirty || !!testDraft} on:click={save}>{busy ? 'Saving…' : 'Save revision'}</button></div></div>{/if}
