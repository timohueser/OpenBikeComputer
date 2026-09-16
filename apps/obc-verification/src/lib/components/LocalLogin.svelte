<script lang="ts">
  export let primary = true;
  let username = '';
  let password = '';
  let busy = false;
  let error = '';

  async function login() {
    busy = true; error = '';
    try {
      const response = await fetch('/api/login', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ username, password }) });
      if (!response.ok) { const body = await response.json(); throw new Error(body.error || 'Sign-in failed.'); }
      window.location.assign('/');
    } catch (e) { error = e instanceof Error ? e.message : 'Unable to sign in. Please try again.'; }
    finally { busy = false; }
  }
</script>

<form on:submit|preventDefault={login}>
  <fieldset disabled={busy}>
    <label>Username<input required autocomplete="username" bind:value={username} /></label>
    <label>Password<input required type="password" autocomplete="current-password" bind:value={password} /></label>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <button class:primary class="full-width">{busy ? 'Signing in…' : 'Sign in as local administrator'}</button>
  </fieldset>
</form>
