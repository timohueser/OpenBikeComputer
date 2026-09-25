<script lang="ts">
  import { onMount, tick } from 'svelte';
  import type { Bootstrap, Revision } from '$lib/types';
  import { isFilter, type Filter } from '$lib/filters';
  import Requirements from '$lib/components/Requirements.svelte';
  import Releases from '$lib/components/Releases.svelte';
  import CoverageOverview from '$lib/components/CoverageOverview.svelte';
  import Account from '$lib/components/Account.svelte';
  import { api, message } from '$lib/components/api';
  type View = 'requirements' | 'coverage' | 'releases' | 'account';
  const views: View[] = ['requirements', 'coverage', 'releases', 'account'];
  let data: Bootstrap | null = null;
  let view: View = 'requirements';
  let previousView: Exclude<View, 'account'> = 'requirements';
  let accountDirty = false;
  let resetCompleted = false;
  let resetBusy = false;
  let error = '';
  let loading = true;
  let requirementsDirty = false;
  let runDirty = false;
  let requirementsView: Requirements;
  /** The requirement and filter of the requirements view. Both live in the URL so a link, a refresh and Back return to them. */
  let selected = '';
  let filter: Filter = 'all';
  let located = false;
  async function load() { loading = true; error = ''; try { data = await api<Bootstrap>('/api/bootstrap'); } catch (e) { error = message(e); } finally { loading = false; } }
  function saved(revision: Revision) { if (data) data = {...data, revision}; }
  function navigate(next: View) {
    if (resetBusy) return;
    if (view === 'account' && next !== 'account' && accountDirty && !confirm('Discard the unsaved password change?')) return;
    if (next === 'account' && view !== 'account') previousView = view as typeof previousView;
    if (next !== 'account') accountDirty = false;
    if (next !== view) { view = next; history.pushState(null, '', address()); focusHeading(); }
  }
  /** Opens the requirements view with a filter, as a coverage tile asks. */
  function showRequirements(next: Filter) { navigate('requirements'); requirementsView?.show('', next); }
  function address() {
    const params = new URLSearchParams({ view });
    if (view === 'requirements' && selected) params.set('req', selected);
    if (view === 'requirements' && filter !== 'all') params.set('filter', filter);
    return `?${params}`;
  }
  function locate() {
    const params = new URLSearchParams(location.search);
    const next = params.get('view');
    view = views.includes(next as View) ? next as View : 'requirements';
    const req = params.get('req') ?? '', f = params.get('filter');
    if (requirementsView) requirementsView.show(req, isFilter(f) ? f : 'all');
    else { selected = req; filter = isFilter(f) ? f : 'all'; }
  }
  $: if (located) { selected; filter; history.replaceState(null, '', address()); }
  async function focusHeading() {
    await tick();
    const heading = [...document.querySelectorAll<HTMLElement>('#main h1')].find(h => h.offsetParent !== null);
    heading?.setAttribute('tabindex', '-1'); heading?.focus();
  }
  async function logout() {
    if (resetBusy) return;
    if ((requirementsDirty || runDirty || accountDirty) && !confirm('Discard unsaved changes and sign out?')) return;
    try { await api('/api/logout', 'POST'); window.location.assign('/login'); } catch (e) { error = message(e); }
  }
  function resetWorkspace() { resetCompleted = true; window.location.assign('/'); }
  function guard(event: BeforeUnloadEvent) { if (!resetCompleted && (resetBusy || requirementsDirty || runDirty || accountDirty)) { event.preventDefault(); event.returnValue = ''; } }
  onMount(async () => { locate(); await load(); located = true; });
</script>
<svelte:head><title>Verification & releases · OpenBikeComputer</title><meta name="description" content="OpenBikeComputer system requirements, verification evidence, and releases." /></svelte:head>
<svelte:window on:beforeunload={guard} on:popstate={() => { locate(); focusHeading(); }} />
<header class="chrome"><a class="brand" href="https://openbikecomputer.com"><img class="brand-icon" src="/brand/app-icon.svg" width="32" height="32" alt="" /><span>OpenBikeComputer<span class="brand-sub">Verification & releases</span></span></a><nav aria-label="Main navigation"><button disabled={resetBusy} aria-current={view === 'requirements' ? 'page' : undefined} on:click={() => navigate('requirements')}>Requirements{#if requirementsDirty}<span class="draft-dot" title="Unsaved changes"></span>{/if}</button><button disabled={resetBusy} aria-current={view === 'coverage' ? 'page' : undefined} on:click={() => navigate('coverage')}>Coverage</button><button disabled={resetBusy} aria-current={view === 'releases' ? 'page' : undefined} on:click={() => navigate('releases')}>Releases{#if runDirty}<span class="draft-dot" title="Unsaved release changes"></span>{/if}</button></nav>{#if data}<div class="identity"><button disabled={resetBusy} class="account-link" aria-current={view === 'account' ? 'page' : undefined} on:click={() => navigate('account')}><span class="account-name">{data.actor.name} · </span>Account</button><button disabled={resetBusy} class="text-button" on:click={logout}>Sign out</button></div>{/if}</header>
<main id="main" class="app-main">{#if data?.configured.demo}<div class="alert warning"><strong>Local coverage demonstration</strong><span>Requirements are copied and edited for this demo. Additional tests and all results are illustrative. Nothing is sent to production.</span></div>{/if}{#if error}<div class="alert error" role="alert">{error} <button on:click={load}>Try again</button></div>{/if}{#if loading}<div class="empty"><p class="muted" role="status">Loading your workspace…</p></div>{:else if data}<div hidden={view !== 'requirements'}><Requirements bind:this={requirementsView} bind:selected bind:filter visible={view === 'requirements'} revision={data.revision} catalog={data.catalog} bind:dirty={requirementsDirty} onsaved={saved} oncatalog={(catalog) => { if (data) data = { ...data, catalog }; }} /></div><div hidden={view !== 'releases'}><Releases actor={data.actor} revision={data.revision} candidates={data.candidates} configured={data.configured.github} {requirementsDirty} bind:dirty={runDirty} /></div>{#if view === 'coverage'}<CoverageOverview revision={data.revision} catalog={data.catalog} onfilter={showRequirements} />{/if}{#if view === 'account'}<Account actor={data.actor} bind:dirty={accountDirty} bind:resetBusy workspaceDirty={requirementsDirty || runDirty} onreset={resetWorkspace} onback={() => navigate(previousView)} />{/if}{/if}</main>
<footer class="site-footer"><span>OpenBikeComputer</span><span>Requirements → verification → release</span></footer>
