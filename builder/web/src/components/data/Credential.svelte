<script lang="ts">
    import { command, setCredential, type CliError } from "../../lib/data/cli";
    import { list, type CredentialSet, type CredentialState } from "../../lib/data/shapes";
    import Failure from "./Failure.svelte";

    // A value lives only in its input until Save sends it, and the form is reset after; no state
    // of the page holds it.
    let { credential, set, onsaved }: { credential: CredentialState; set: string[]; onsaved: () => void } = $props();

    let replacing = $state(false);
    const open = $derived(credential.state !== "ready" || replacing);
    let saving = $state(false);
    let error = $state<CliError | null>(null);
    let saved = $state<CredentialSet | null>(null);
    const site = $derived(credential.register && new URL(credential.register).host);

    async function save(event: SubmitEvent) {
        event.preventDefault();
        const form = event.currentTarget as HTMLFormElement;
        const values = credential.fields.map((field) => (form.elements.namedItem(field) as HTMLInputElement).value.trim());
        saving = true;
        error = null;
        const result = await setCredential<CredentialSet>(set, values);
        saving = false;
        if (!result.ok) {
            error = result.error;
            return;
        }
        form.reset();
        saved = result.output;
        replacing = false;
        onsaved();
    }
</script>

<div class="credential">
    {#if credential.uses.length}<p class="soft">For {credential.uses.join("; ")}.</p>{/if}
    {#if saved}
        <p class="ready" role="status">Saved {list(saved.names)} in <code>{saved.file}</code>.</p>
        {#if saved.overridden.length}
            <p class="missing">
                The environment of the page also sets {list(saved.overridden)}, and the environment wins. Remove it from tools/obc.local, then
                start obc data ui again.
            </p>
        {/if}
    {/if}
    {#if open}
        <form onsubmit={save}>
            {#if credential.hint || site}
                <p>{credential.hint}{#if site}{" "}<a href={credential.register} target="_blank" rel="noreferrer">Open {site}</a>{/if}</p>
            {/if}
            {#each credential.fields as field (field)}
                <label>
                    <code>{field}</code>
                    <input name={field} type="password" required autocomplete="off" spellcheck="false" />
                </label>
            {/each}
            <div class="title">
                <button type="submit" class="primary" disabled={saving} aria-busy={saving}>Save</button>
                {#if replacing}<button type="button" onclick={() => (replacing = false)}>Cancel</button>{/if}
                <details class="terminal">
                    <summary>In a terminal</summary>
                    <code>{command(set)}</code> reads each value on a line of its own.
                </details>
            </div>
            {#if error}<Failure {error} />{/if}
        </form>
    {:else}
        <button
            type="button"
            onclick={() => {
                replacing = true;
                saved = null;
            }}>Replace</button
        >
    {/if}
</div>

<style>
    .credential {
        flex-basis: 100%;
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 8px;
        padding: 0 0 8px 30px;
    }
    form {
        display: flex;
        flex-direction: column;
        gap: 8px;
        align-self: stretch;
    }
    label {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 4px 8px;
    }
    /* One width for every name, so that the inputs line up; on a phone the input wraps below. */
    label code {
        flex: 0 0 210px;
    }
    input {
        flex: 1 1 220px;
        min-width: 0;
        min-height: 40px;
        padding: 0 10px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        font: inherit;
    }
    .terminal summary {
        color: var(--ink-soft);
        cursor: pointer;
    }
    .terminal code {
        padding: 2px 6px;
        background: var(--panel);
        border: 1px solid var(--line-strong);
        border-radius: 6px;
    }
</style>
