<script lang="ts">
  import { onMount } from 'svelte';
  import type { AgentToken } from '$lib/types';
  import { api, date, message } from './api';

  export let busy = false;
  let tokens: AgentToken[] = [];
  let loading = true;
  let name = '';
  let lifetimeMinutes = 60;
  let fresh: { token: string; access: AgentToken } | undefined;
  let showInactive = false;
  let now = Date.now();
  let error = '';
  let notice = '';
  const status = (token: AgentToken, time: number) => token.revokedAt ? 'Revoked' : Date.parse(token.expiresAt) <= time ? 'Expired' : 'Active';
  $: visible = tokens.filter(token => showInactive || status(token, now) === 'Active');

  async function load() {
    loading = true; error = '';
    try { tokens = await api<AgentToken[]>('/api/admin/agent-tokens'); }
    catch (e) { error = message(e); }
    finally { loading = false; }
  }
  async function create() {
    busy = true; error = ''; notice = '';
    try {
      fresh = await api('/api/admin/agent-tokens', 'POST', { name, lifetimeMinutes });
      tokens = [fresh!.access, ...tokens]; name = '';
    } catch (e) { error = message(e); }
    finally { busy = false; }
  }
  async function revoke(token: AgentToken) {
    busy = true; error = ''; notice = '';
    try {
      await api(`/api/admin/agent-tokens/${encodeURIComponent(token.id)}`, 'DELETE');
      tokens = tokens.map(value => value.id === token.id ? { ...value, revokedAt: new Date().toISOString() } : value);
      if (fresh?.access.id === token.id) fresh = undefined;
      notice = `Access revoked for ${token.name}. Its proposals remain available for review.`;
    } catch (e) { error = message(e); }
    finally { busy = false; }
  }
  async function copy() {
    try { await navigator.clipboard.writeText(fresh!.token); notice = 'Token copied. Save it outside the repository.'; }
    catch { error = 'Copy failed. Select the token and copy it, or download the token file.'; }
  }
  function download() {
    const url = URL.createObjectURL(new Blob([fresh!.token + '\n'], { type: 'text/plain' }));
    const link = document.createElement('a');
    link.href = url; link.download = 'verification-agent.token'; link.click();
    URL.revokeObjectURL(url);
  }
  onMount(() => {
    void load();
    const timer = setInterval(() => { now = Date.now(); }, 1000);
    return () => clearInterval(timer);
  });
</script>

<div class="row"><h2>Agent access</h2><button disabled={loading || busy} on:click={load}>Refresh tokens</button></div>
<p class="muted">Agents can read requirements and test information, and propose coverage plans. A person must approve each proposal. Tokens cannot edit requirements, record results, or publish releases.</p>
{#if error}<div class="alert error" role="alert">{error}</div>{/if}
{#if notice}<div class="alert success" role="status">{notice}</div>{/if}
{#if fresh}
  <section class="inset" aria-label="New agent token">
    <h3>Save this token now</h3>
    <p>This is the only time you can copy or download it. Save it before leaving this tab. It expires {date(fresh.access.expiresAt)}.</p>
    <label>New token<input readonly value={fresh.token} autocomplete="off" spellcheck={false} /></label>
    <div class="actions"><button on:click={copy}>Copy token</button><button on:click={download}>Download token file</button><button on:click={() => { fresh = undefined; notice = ''; }}>Done — hide token</button></div>
    <p class="small muted">Save as <code class="wrap">~/.config/openbikecomputer/verification-agent.token</code> with owner-only permissions (0600). Give the agent the file path. Keep the token out of Git, chat, and logs.</p>
  </section>
{/if}
<form class="inset" on:submit|preventDefault={create}>
  <h3>Create a temporary token</h3>
  <fieldset disabled={busy || loading || !!fresh}>
    <label>Token name<input required maxlength={100} bind:value={name} placeholder="Laptop: requirement coverage" autocomplete="off" /></label>
    <label>Lifetime in minutes<input type="number" required min={1} max={240} step={1} bind:value={lifetimeMinutes} /></label>
    <p class="small muted">Maximum 4 hours. Tokens expire automatically and can be revoked at any time.</p>
    <button class="primary">Create token</button>
  </fieldset>
</form>
<label class="check"><input type="checkbox" bind:checked={showInactive} />Show expired and revoked tokens</label>
{#if loading}<p class="muted" role="status">Loading tokens…</p>
{:else}
  {#each visible as token (token.id)}
    <article class="test wrap" aria-label={`Token: ${token.name}`}>
      <div class="row"><h3>{token.name}</h3><span class="badge">{status(token, now)}</span></div>
      <p class="small muted">Created by {token.issuedBy.name} · {date(token.createdAt)}<br />Expires {date(token.expiresAt)} · Last used {token.lastUsedAt ? date(token.lastUsedAt) : 'never'}</p>
      {#if token.revokedAt}<p class="small muted">Revoked {date(token.revokedAt)}</p>
      {:else if status(token, now) === 'Active'}<button class="danger" disabled={busy} on:click={() => revoke(token)}>Revoke token</button>{/if}
    </article>
  {:else}<p class="muted">{showInactive ? 'No agent tokens have been created.' : 'No active agent tokens.'}</p>{/each}
{/if}
