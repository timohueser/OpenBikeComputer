<script lang="ts">
  import type { Revision, Requirement, Catalog, VerificationTest, LinkProposal } from '$lib/types';
  import { api, clone, date, message } from './api';
  import Markdown from './Markdown.svelte';
  import MarkdownField from './MarkdownField.svelte';
  import TestEditor from './TestEditor.svelte';
  import Files from './Files.svelte';
  import RequirementGroups from './RequirementGroups.svelte';
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
  let search = '';
  let query = '';
  let groupFilter = 'all';
  let manageGroups = false;
  let busy = false;
  let error = '';
  let notice = '';
  let history: Revision[] | null = null;
  let historical: Revision | null = null;
  let proposals: LinkProposal[] | null = null;
  let undo: { requirementId: string; test: VerificationTest; index: number } | null = null;
  $: testChanged = testDraft !== null && JSON.stringify(testDraft) !== JSON.stringify(requirement?.tests.find(t => t.id === testDraft?.id));
  $: dirty = (requirement, JSON.stringify(requirements) !== JSON.stringify(revision.requirements) || testChanged);
  $: requirement = requirements.find(r => r.id === selected);
  $: groups = [...new Set(requirements.map(groupName).filter(Boolean))].sort((a, b) => a.localeCompare(b)).map(name => ({ name, count: requirements.filter(r => groupName(r) === name).length }));
  $: filtered = requirements.filter(r => matches(r, groupFilter, query));
  $: sections = [...new Set(filtered.map(groupName))].sort((a, b) => a ? b ? a.localeCompare(b) : -1 : 1).map(name => ({ name, requirements: filtered.filter(r => groupName(r) === name) }));
  $: matching = catalog.cases.filter(c => `${c.name} ${c.suite} ${c.id} ${c.file || ''}`.toLowerCase().includes(search.toLowerCase()));
  function groupName(r: Requirement) { return r.group?.trim() || ''; }
  function matches(r: Requirement, filter: string, term: string) {
    const inGroup = filter === 'all' || groupName(r) === (filter === 'ungrouped' ? '' : filter.slice(6));
    return inGroup && `${r.id} ${r.title} ${r.statement} ${groupName(r)}`.toLowerCase().includes(term.toLowerCase());
  }
  function filterList(filter: string, term: string) {
    const next = requirements.filter(r => matches(r, filter, term));
    if (!next.some(r => r.id === selected)) {
      if (!leaveEditor()) return false;
      selected = next[0]?.id || '';
    }
    groupFilter = filter; query = term; return true;
  }
  function setGroup(value: string) {
    if (busy || !requirement) return;
    update({ ...requirement, group: value });
    const changed = { ...requirement, group: value };
    if (!matches(changed, groupFilter, '')) groupFilter = 'all';
    if (!matches(changed, 'all', query)) query = '';
  }
  function renameGroup(name: string, replacement: string) {
    if (busy) return;
    requirements = requirements.map(r => groupName(r) === name ? { ...r, group: replacement } : r);
    if (groupFilter === `group:${name}`) groupFilter = `group:${replacement}`;
    query = '';
  }
  function removeGroup(name: string) {
    if (busy) return;
    requirements = requirements.map(r => groupName(r) === name ? { ...r, group: undefined } : r);
    if (groupFilter === `group:${name}`) groupFilter = 'ungrouped';
    query = '';
  }
  function leaveEditor() {
    if (testChanged && !confirm('Discard the unsaved manual test changes?')) return false;
    testDraft = null; edit = false; picker = false; return true;
  }
  function select(id: string) { if (leaveEditor()) selected = id; }
  function create() {
    if (!leaveEditor()) return;
    let n = 1; while (requirements.some(r => r.id === `SYS-${String(n).padStart(3, '0')}`)) n++;
    const r: Requirement = { ...(groupFilter.startsWith('group:') ? { group: groupFilter.slice(6) } : {}), id: `SYS-${String(n).padStart(3, '0')}`, title: '', statement: '', active: true, tests: [] };
    requirements = [...requirements, r]; selected = r.id; query = ''; edit = true;
  }
  function update(r: Requirement) { requirements = requirements.map(value => value.id === r.id ? r : value); }
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
  async function save() {
    error = ''; notice = '';
    if (testDraft) { error = 'Keep or cancel the manual test changes before saving the revision.'; return; }
    if (requirements.some(r => !r.title.trim() || !r.statement.trim())) { error = 'Every requirement needs a title and statement before you save.'; return; }
    busy = true;
    try {
      const saved = await api<Revision>('/api/requirements', 'PUT', { baseRevision: revision.id, requirements });
      revision = saved; requirements = clone(saved.requirements); undo = null; edit = false; onsaved(saved); notice = `Revision r${saved.id} saved. Existing candidates keep their original revision.`;
    } catch (e) { error = message(e); } finally { busy = false; }
  }
  async function refresh() {
    if (dirty && !confirm('Discard this draft and load the latest saved revision?')) return;
    busy = true; error = '';
    try { const versions = await api<Revision[]>('/api/revisions'); const latest = versions.sort((a,b) => b.id - a.id)[0]; if (latest) { revision = latest; requirements = clone(latest.requirements); groupFilter = 'all'; query = ''; selected = requirements[0]?.id || ''; testDraft = null; edit = false; undo = null; onsaved(latest); } }
    catch (e) { error = message(e); } finally { busy = false; }
  }
  async function refreshCatalog() {
    busy = true; error = '';
    try { catalog = await api<Catalog>('/api/catalog'); oncatalog(catalog); } catch (e) { error = message(e); } finally { busy = false; }
  }
  async function showHistory() { error = ''; try { history = await api<Revision[]>('/api/revisions'); } catch (e) { error = message(e); } }
  async function showProposals() { error = ''; try { proposals = await api<LinkProposal[]>('/api/proposals'); } catch (e) { error = message(e); } }
  async function decide(id: string, accept: boolean) {
    if (dirty) { error = 'Save or discard your draft before reviewing a link proposal.'; return; }
    busy = true; error = '';
    try { await api('/api/proposals/' + id, 'POST', { accept }); await refresh(); await showProposals(); } catch (e) { error = message(e); } finally { busy = false; }
  }
</script>
<div class="page-heading row"><div><div class="eyebrow">Product verification</div><h1>Requirements</h1><p class="muted">The promises we make, and how we check them.</p></div><div class="actions"><button on:click={() => manageGroups = !manageGroups}>Manage groups</button><button on:click={showProposals}>Link proposals</button><button on:click={showHistory}>History</button><button class="primary" on:click={create}>+ Requirement</button></div></div>
{#if error}<div class="alert error" role="alert">{error}<button class="text-button" disabled={busy} on:click={refresh}>Reload saved revision</button></div>{/if}
{#if notice}<div class="alert success" role="status">{notice}</div>{/if}
{#if manageGroups}<RequirementGroups {groups} disabled={busy} onrename={renameGroup} onremove={removeGroup} onclose={() => manageGroups = false} />{/if}
{#if proposals !== null}
  <section class="panel"><div class="row"><h2>Proposed verification links</h2><button on:click={() => proposals = null}>Close</button></div><p class="muted small">Agents can propose links. Only your explicit approval changes a saved revision.</p>
    {#each proposals.filter(p => p.status === 'pending') as p}<div class="test"><div class="row"><h3>{p.action === 'add' ? 'Link' : 'Unlink'} · {p.requirementId}</h3><span class="small muted">r{p.baseRevision} · {p.author}</span></div><code class="wrap">{p.caseId}</code><p>{p.reason}</p><div class="actions"><button class="primary" disabled={busy || dirty || p.baseRevision !== revision.id} on:click={() => decide(p.id, true)}>Approve link change</button><button disabled={busy} on:click={() => decide(p.id, false)}>Reject</button>{#if p.baseRevision !== revision.id}<span class="small warning">Based on an older revision. Request a fresh proposal.</span>{/if}</div></div>{:else}<p class="muted">No proposals awaiting your review.</p>{/each}
  </section>
{/if}
{#if history !== null}
  <section class="panel"><div class="row"><h2>Revision history</h2><button on:click={() => { history = null; historical = null; }}>Close</button></div>
    <div class="history-list">{#each [...history].sort((a,b) => b.id-a.id) as r}<button class:selected={historical?.id === r.id} on:click={() => historical = r}>r{r.id} · {date(r.createdAt)} · {r.author}</button>{/each}</div>
    {#if historical}<div class="section"><p class="eyebrow">Read-only revision r{historical.id}</p>{#each historical.requirements as r}<details class="test"><summary>{r.id} · {r.title} {r.active ? '' : '(inactive)'} {#if r.todo}<span class="warning">· To do</span>{/if}</summary><p class="small muted">Group: {groupName(r) || 'Ungrouped'}</p><Markdown text={r.statement} />{#each r.tests as t}<div class="inset"><h3>{t.title} <span class="muted small">· {t.kind}</span></h3>{#if t.kind === 'manual'}<Markdown text={t.steps || ''} /><Markdown text={t.expected || ''} /><Files files={t.inputs} label="Input files" />{:else}<code class="wrap">{t.caseId}</code>{/if}</div>{/each}</details>{/each}</div>{/if}
  </section>
{/if}
<div class="workbench"><aside>
  <label class="search-label">Find a requirement<input type="search" value={query} on:input={(event) => { if (!filterList(groupFilter, event.currentTarget.value)) event.currentTarget.value = query; }} placeholder="Search requirements…" /></label>
  <label class="search-label">Group<select value={groupFilter} on:change={(event) => { if (!filterList(event.currentTarget.value, query)) event.currentTarget.value = groupFilter; }}>
    <option value="all">All groups</option><option value="ungrouped">Ungrouped</option>{#each groups as group}<option value={'group:' + group.name}>{group.name}</option>{/each}
  </select></label>
  <div class="muted small sidebar-caption">{filtered.length} of {requirements.length} requirements · r{revision.id}</div>
  {#each sections as section}
    <h3 class="group-heading">{section.name || 'Ungrouped'} <span>{section.requirements.length}</span></h3>
    {#each section.requirements as r}<button class="entry" class:selected={selected === r.id} on:click={() => select(r.id)}><span class="eyebrow">{r.id} {#if !r.active}· inactive{/if}</span><strong>{r.title || 'Untitled requirement'} {#if r.todo}<span class="todo-label">To do</span>{/if}</strong><span class="small muted">{r.tests.length} {r.tests.length === 1 ? 'check' : 'checks'}{r.active && !r.tests.length ? ' · verification missing' : ''}</span></button>{/each}
  {:else}<p class="muted small">{requirements.length ? 'No matching requirements.' : 'Start with one important product promise.'}</p>{/each}
</aside>
<section class="detail">
{#if requirement}
  <div class="row"><span class="eyebrow">{requirement.id} · {groupName(requirement) || 'Ungrouped'}</span><div class="actions">{#if requirement.todo}<span class="badge warning">To do · definition incomplete</span>{/if}<span class:warning={!requirement.active} class="badge">{requirement.active ? 'Active for future candidates' : 'Inactive draft'}</span>{#if !edit && !testDraft && !picker}<button on:click={() => edit = true}>Edit requirement</button>{/if}</div></div>
  {#if edit}<div class="section"><label>Title<input bind:value={requirement.title} on:input={() => requirements = [...requirements]} placeholder="A clear product promise" /></label><label>Group <span class="small muted">· Optional</span><input disabled={busy} list="requirement-group-names" maxlength={80} value={requirement.group || ''} on:input={(event) => setGroup(event.currentTarget.value)} placeholder="Choose an existing group or type a new name" /><datalist id="requirement-group-names">{#each groups as group}<option value={group.name}></option>{/each}</datalist></label><MarkdownField label="Requirement" bind:value={requirement.statement} /><label class="check"><input type="checkbox" disabled={busy} checked={!!requirement.todo} on:change={(event) => update({ ...requirement!, todo: event.currentTarget.checked || undefined })} />To do — definition incomplete</label><p class="small muted">An active requirement marked To do blocks release publication. An exception cannot override it. Uncheck when the definition is complete.</p><label class="check"><input type="checkbox" bind:checked={requirement.active} />Include in future release candidates</label><p class="muted small">Inactive requirements remain visible but do not block a release. Existing candidates never change.</p><button on:click={() => edit = false}>Done editing</button></div>
  {:else}<h2 class="requirement-title">{requirement.title || 'Untitled requirement'}</h2><div class="statement"><Markdown text={requirement.statement} /></div>{/if}
  {#if testDraft}<div class="section"><TestEditor bind:test={testDraft} onsave={keepTest} oncancel={() => { if (!testChanged || confirm('Discard these manual test changes?')) testDraft = null; }} /></div>
  {:else if picker}<div class="section"><div class="row"><h2>Link an automated test</h2><div class="actions"><button disabled={busy} on:click={refreshCatalog}>Refresh catalogue</button><button on:click={() => picker = false}>Back to requirement</button></div></div><p class="muted small">Select a real test from the latest CI catalogue. CI still runs complete suites.</p><input type="search" aria-label="Search automated tests" bind:value={search} placeholder="Search test name, suite, or file…" />{#if catalog.sourceSha}<p class="muted small">Catalogue from <code>{catalog.sourceSha.slice(0, 10)}</code> · {date(catalog.updatedAt)}</p>{/if}<div class="catalog">{#if matching.length > 100}<p class="small muted">Showing the first 100 of {matching.length} matches. Refine your search to find a specific test.</p>{/if}{#each matching.slice(0, 100) as c}<div class="test"><div class="row"><h3>{c.name}</h3><button disabled={requirement.tests.some(t => t.caseId === c.id)} on:click={() => link(c.id, c.name)}>{requirement.tests.some(t => t.caseId === c.id) ? 'Linked' : 'Link test'}</button></div><div class="small muted">{c.suite}{c.file ? ' · ' + c.file : ''}</div><code class="wrap small">{c.id}</code></div>{:else}<div class="empty"><h3>{catalog.cases.length ? 'No matching tests' : 'No CI catalogue yet'}</h3><p class="muted">{catalog.cases.length ? 'Try a test name or suite name.' : 'The verification workflow imports real test results. After its first run, refresh the catalogue to browse available tests.'}</p></div>{/each}</div></div>
  {:else}<div class="section"><div class="row"><h2>Verification <span class="muted">{requirement.tests.length}</span></h2><div class="actions"><button on:click={() => picker = true}>Link automated test</button><button on:click={() => testDraft = { id: crypto.randomUUID(), kind: 'manual', title: '', steps: '', expected: '', inputs: [] }}>+ Manual test</button></div></div>
    {#if undo}<div class="alert" role="status">Test removed from draft. <button class="text-button" on:click={restoreTest}>Undo</button></div>{/if}
    {#each requirement.tests as t}<div class="test"><div class="row"><div><div class="eyebrow">{t.kind}</div><h3>{t.title}</h3></div><div class="actions">{#if t.kind === 'manual'}<button on:click={() => testDraft = clone(t)}>Edit test</button>{/if}<button class="text-button danger" on:click={() => removeTest(t)}>{t.kind === 'manual' ? 'Remove test' : 'Unlink'}</button></div></div>{#if t.kind === 'automated'}<code class="wrap small">{t.caseId}</code>{#if !catalog.cases.some(c => c.id === t.caseId)}<p class="warning small">This test is absent from the latest catalogue. Check the link before preparing a release.</p>{/if}{:else}<details><summary>Procedure and input files</summary><Markdown text={t.steps || ''} /><h4>Expected result</h4><Markdown text={t.expected || ''} /><Files files={t.inputs} label="Input files" /></details>{/if}</div>{:else}<div class="empty"><h3>How will we verify this?</h3><p class="muted">Link an automated test or add a manual procedure. An active requirement without checks blocks publication.</p></div>{/each}</div>{/if}
{:else}<div class="empty"><h2>{requirements.length ? 'No matching requirements' : 'Make the important promises explicit.'}</h2><p class="muted">{requirements.length ? 'Choose another group or clear your search.' : 'Write a measurable requirement, then define the checks that provide evidence.'}</p><button class="primary" on:click={create}>Create requirement</button></div>{/if}
</section></div>
<div class="savebar row"><span class="small">{dirty ? 'You have unsaved changes' : `Saved revision r${revision.id}`} <span class="muted">· Candidates use saved revisions only.</span></span><div class="actions">{#if dirty}<button disabled={busy} on:click={refresh}>Discard draft</button>{/if}<button class="primary" disabled={busy || !dirty || !!testDraft} on:click={save}>{busy ? 'Saving…' : 'Save revision'}</button></div></div>
