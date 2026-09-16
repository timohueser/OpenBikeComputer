<script lang="ts">
  import { onMount } from 'svelte';
  import type { Bootstrap, Revision } from '$lib/types';
  import Requirements from '$lib/components/Requirements.svelte';
  import Releases from '$lib/components/Releases.svelte';
  import { api, message } from '$lib/components/api';
  let data: Bootstrap | null = null;
  let view: 'requirements' | 'releases' = 'requirements';
  let error = '';
  let loading = true;
  let requirementsDirty = false;
  let runDirty = false;
  async function load() { loading = true; error = ''; try { data = await api<Bootstrap>('/api/bootstrap'); } catch (e) { error = message(e); } finally { loading = false; } }
  function saved(revision: Revision) { if (data) data = {...data, revision}; }
  function navigate(next: typeof view) { view = next; }
  async function logout() {
    if ((requirementsDirty || runDirty) && !confirm('Discard unsaved changes and sign out?')) return;
    try { await api('/api/logout', 'POST'); window.location.assign('/login'); } catch (e) { error = message(e); }
  }
  function guard(event: BeforeUnloadEvent) { if (requirementsDirty || runDirty) { event.preventDefault(); event.returnValue = ''; } }
  onMount(load);
</script>
<svelte:head><title>Verification & releases · OpenBikeComputer</title><meta name="description" content="OpenBikeComputer system requirements, verification evidence, and releases." /></svelte:head>
<svelte:window on:beforeunload={guard} />
<header class="chrome"><a class="brand" href="https://openbikecomputer.com"><span class="brand-icon">↗</span><span>OpenBikeComputer<span class="brand-sub">Verification & releases</span></span></a><nav aria-label="Main navigation"><button aria-current={view === 'requirements' ? 'page' : undefined} on:click={() => navigate('requirements')}>Requirements{#if requirementsDirty}<span class="draft-dot" title="Unsaved changes"></span>{/if}</button><button aria-current={view === 'releases' ? 'page' : undefined} on:click={() => navigate('releases')}>Releases{#if runDirty}<span class="draft-dot" title="Unsaved result"></span>{/if}</button></nav>{#if data}<div class="identity"><span class="small muted">{data.actor.name}</span><button class="text-button" on:click={logout}>Sign out</button></div>{/if}</header>
<main class="app-main">{#if error}<div class="alert error" role="alert">{error} <button on:click={load}>Try again</button></div>{/if}{#if loading}<div class="empty"><p class="muted" role="status">Loading your workspace…</p></div>{:else if data}<div hidden={view !== 'requirements'}><Requirements revision={data.revision} catalog={data.catalog} bind:dirty={requirementsDirty} onsaved={saved} oncatalog={(catalog) => { if (data) data = { ...data, catalog }; }} /></div><div hidden={view !== 'releases'}><Releases revision={data.revision} candidates={data.candidates} configured={data.configured.github} {requirementsDirty} bind:dirty={runDirty} /></div>{/if}</main>
<footer class="site-footer"><span>OpenBikeComputer</span><span>Requirements → verification → release</span></footer>
