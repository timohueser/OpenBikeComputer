<script lang="ts">
  import { onMount } from 'svelte';
  let username = '';
  let password = '';
  let oauth = false;
  let busy = false;
  let error = '';
  onMount(async () => { try { const response = await fetch('/api/login'); if (response.ok) oauth = (await response.json()).oauth; } catch { /* Local login remains available. */ } });
  async function login() {
    busy = true; error = '';
    try {
      const response = await fetch('/api/login', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ username, password }) });
      if (!response.ok) { const body = await response.json(); throw new Error(body.error || 'Sign-in failed.'); }
      window.location.assign('/');
    } catch (e) { error = e instanceof Error ? e.message : 'Unable to sign in. Please try again.'; } finally { busy = false; }
  }
</script>
<svelte:head><title>Sign in · OpenBikeComputer verification</title></svelte:head>
<main class="login-shell"><a class="brand" href="https://openbikecomputer.com"><span class="brand-icon">↗</span>OpenBikeComputer</a><section class="login-card"><div class="eyebrow">Maintainer workspace</div><h1>Verification & releases</h1><p class="muted">A clear view of what must work, and the evidence behind every release.</p>{#if oauth}<a href="/auth/github" class="button primary full-width">Continue with GitHub</a><div class="login-divider muted small">or use your owner account</div>{/if}<form on:submit|preventDefault={login}><label>Username<input required autocomplete="username" bind:value={username} /></label><label>Password<input required type="password" autocomplete="current-password" bind:value={password} /></label>{#if error}<p class="error" role="alert">{error}</p>{/if}<button class="primary full-width" disabled={busy}>{busy ? 'Signing in…' : 'Sign in'}</button></form><p class="small muted">Access is restricted to approved maintainers.</p></section><a class="small muted" href="https://openbikecomputer.com">← OpenBikeComputer website</a></main>
