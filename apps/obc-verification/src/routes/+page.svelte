<script lang="ts">
  import { onMount } from 'svelte';
  import type { Bootstrap, Revision } from '$lib/types';
  import Requirements from '$lib/components/Requirements.svelte';
  import Releases from '$lib/components/Releases.svelte';
  import Account from '$lib/components/Account.svelte';
  import { api, message } from '$lib/components/api';
  let data: Bootstrap | null = null;
  let view: 'requirements' | 'releases' | 'account' = 'requirements';
  let previousView: 'requirements' | 'releases' = 'requirements';
  let accountDirty = false;
  let resetCompleted = false;
  let resetBusy = false;
  let error = '';
  let loading = true;
  let requirementsDirty = false;
  let runDirty = false;
  async function load() { loading = true; error = ''; try { data = await api<Bootstrap>('/api/bootstrap'); } catch (e) { error = message(e); } finally { loading = false; } }
  function saved(revision: Revision) { if (data) data = {...data, revision}; }
  function navigate(next: typeof view) {
    if (resetBusy) return;
    if (view === 'account' && next !== 'account' && accountDirty && !confirm('Discard the unsaved password change?')) return;
    if (next === 'account' && view !== 'account') previousView = view;
    if (next !== 'account') accountDirty = false;
    view = next;
  }
  async function logout() {
    if (resetBusy) return;
    if ((requirementsDirty || runDirty || accountDirty) && !confirm('Discard unsaved changes and sign out?')) return;
    try { await api('/api/logout', 'POST'); window.location.assign('/login'); } catch (e) { error = message(e); }
  }
  function resetWorkspace() { resetCompleted = true; window.location.assign('/'); }
  function guard(event: BeforeUnloadEvent) { if (!resetCompleted && (resetBusy || requirementsDirty || runDirty || accountDirty)) { event.preventDefault(); event.returnValue = ''; } }
  onMount(load);
</script>
<svelte:head><title>Verification & releases · OpenBikeComputer</title><meta name="description" content="OpenBikeComputer system requirements, verification evidence, and releases." /></svelte:head>
<svelte:window on:beforeunload={guard} />
<header class="chrome"><a class="brand" href="https://openbikecomputer.com"><span class="brand-icon">↗</span><span>OpenBikeComputer<span class="brand-sub">Verification & releases</span></span></a><nav aria-label="Main navigation"><button disabled={resetBusy} aria-current={view === 'requirements' ? 'page' : undefined} on:click={() => navigate('requirements')}>Requirements{#if requirementsDirty}<span class="draft-dot" title="Unsaved changes"></span>{/if}</button><button disabled={resetBusy} aria-current={view === 'releases' ? 'page' : undefined} on:click={() => navigate('releases')}>Releases{#if runDirty}<span class="draft-dot" title="Unsaved release changes"></span>{/if}</button></nav>{#if data}<div class="identity"><button disabled={resetBusy} class="account-link" aria-current={view === 'account' ? 'page' : undefined} on:click={() => navigate('account')}><span class="account-name">{data.actor.name} · </span>Account</button><button disabled={resetBusy} class="text-button" on:click={logout}>Sign out</button></div>{/if}</header>
<main class="app-main">{#if data?.configured.demo}<div class="alert warning"><strong>Local coverage demonstration</strong><span>Requirements are copied for this demo. Test results are simulated. Nothing is sent to production.</span></div>{/if}{#if error}<div class="alert error" role="alert">{error} <button on:click={load}>Try again</button></div>{/if}{#if loading}<div class="empty"><p class="muted" role="status">Loading your workspace…</p></div>{:else if data}<div hidden={view !== 'requirements'}><Requirements revision={data.revision} catalog={data.catalog} bind:dirty={requirementsDirty} onsaved={saved} oncatalog={(catalog) => { if (data) data = { ...data, catalog }; }} /></div><div hidden={view !== 'releases'}><Releases actor={data.actor} revision={data.revision} candidates={data.candidates} configured={data.configured.github} {requirementsDirty} bind:dirty={runDirty} /></div>{#if view === 'account'}<Account actor={data.actor} bind:dirty={accountDirty} bind:resetBusy workspaceDirty={requirementsDirty || runDirty} onreset={resetWorkspace} onback={() => navigate(previousView)} />{/if}{/if}</main>
<footer class="site-footer"><span>OpenBikeComputer</span><span>Requirements → verification → release</span></footer>
