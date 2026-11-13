<script lang="ts">
    import { onMount } from 'svelte';
    import Segmented from '../lib/ui/Segmented.svelte';
    import Window from '../lib/ui/Window.svelte';
    import { saveBlob } from '../lib/download';
    import { router } from '../lib/router.svelte';
    import { loadBackup, saveBackup } from '../lib/library/backup';
    import type { Library } from '../lib/library/store';
    import RiderValue from './RiderValue.svelte';
    import { RANGES, readSettings, settings, updateSettings, type Units } from '../lib/settings/settings.svelte';
    import { heartRateZones, powerZones } from '../lib/settings/zones';
    import { appearance, setLook, type Look } from '../lib/theme.svelte';

    let { library }: { library: Library } = $props();

    const units: { value: Units; label: string }[] = [{ value: 'km', label: 'Kilometres' }, { value: 'mi', label: 'Miles' }];
    const looks: { value: Look; label: string }[] = [{ value: 'system', label: 'As the computer' }, { value: 'light', label: 'Light' }, { value: 'dark', label: 'Dark' }];
    const values = [
        { key: 'maxHr', label: 'Maximum heart rate', unit: 'bpm', zones: heartRateZones },
        { key: 'ftp', label: 'Threshold power (FTP)', unit: 'W', zones: powerZones },
    ] as const;

    let counts = $state<{ routes: number; rides: number } | null>(null);
    let message = $state('');
    let busy = $state(false);
    let confirming = $state(false);
    let failure = $state('');
    let file: HTMLInputElement;
    const heading = $props.id();

    const plural = (count: number, word: string) => `${count} ${word}${count === 1 ? '' : 's'}`;
    const text = (error: unknown) => error instanceof Error ? error.message : String(error);

    async function count() {
        const [routes, rides] = await Promise.all([library.routes(), library.rides()]);
        counts = { routes: routes.length, rides: rides.length };
    }
    onMount(() => { count().catch(error => message = text(error)); });

    const setValue = (key: 'maxHr' | 'ftp', value: number | null) =>
        updateSettings({ [key]: value === null ? null : { value, at: Math.floor(Date.now() / 1000), device: false } });

    async function run(work: () => Promise<string | void>) {
        busy = true;
        message = '';
        try {
            message = await work() ?? '';
            await count();
        } catch (error) {
            message = text(error);
        } finally {
            busy = false;
        }
    }

    const save = () => run(async () => {
        saveBlob(await saveBackup(library, $state.snapshot(settings)), `openbikecomputer-${new Date().toLocaleDateString('en-CA')}.zip`);
    });

    function load(chosen: File | undefined) {
        file.value = '';
        if (!chosen) return;
        void run(async () => {
            const added = await loadBackup(library, chosen);
            if (added.settings) updateSettings(readSettings(added.settings));
            return `Added ${plural(added.routes, 'route')} and ${plural(added.rides, 'ride')}.`;
        });
    }

    async function deleteAll() {
        busy = true;
        failure = '';
        try {
            await library.clear();
            location.reload();
        } catch (error) {
            failure = text(error);
            busy = false;
        }
    }
</script>

<Window title="Settings" large onClose={() => router.closeSettings()}>
    <div class="row">
        <strong>Units</strong>
        <Segmented label="Units" options={units} value={settings.units} onChange={value => updateSettings({ units: value })} />
    </div>
    <div class="row">
        <strong>Look</strong>
        <Segmented label="Look" options={looks} value={appearance.look} onChange={setLook} />
    </div>
    {#each values as { key, label, unit, zones } (key)}
        <RiderValue id={key} {label} {unit} value={settings[key]} range={RANGES[key]} {zones} onSet={value => setValue(key, value)} />
    {/each}
    <p class="note">A new ride from the OpenBikeComputer brings the values of the device, and they replace these. A ride keeps its values when you change them later.</p>
    <section aria-labelledby={heading}>
        <h3 id={heading}>Your library</h3>
        <p class="count">
            {#if counts}<strong>{plural(counts.routes, 'route')} and {plural(counts.rides, 'ride')}</strong> are saved in this browser only.{/if}
        </p>
        <div class="buttons">
            <button type="button" onclick={save} disabled={busy || !counts || counts.routes + counts.rides === 0}>Save a backup file</button>
            <button type="button" onclick={() => file.click()} disabled={busy}>Load a backup file</button>
            <input bind:this={file} type="file" accept=".zip,application/zip" hidden onchange={event => load(event.currentTarget.files?.[0])} />
            <button type="button" class="danger" onclick={() => confirming = true} disabled={busy}>Delete all</button>
        </div>
        <p class="message" role="status">{message}</p>
    </section>
</Window>

{#if confirming}
    <Window title="Delete all?" onClose={() => { confirming = false; failure = ''; }}>
        {#snippet children(close)}
        <p>Your routes and rides leave this browser. The device keeps its own, and its rides come in again at the next connection. Close other tabs of this app first.</p>
        {#if failure}<p class="message" role="alert">{failure}</p>{/if}
        <div class="window-buttons">
            <button type="button" onclick={close}>Cancel</button>
            <button type="button" class="danger" onclick={deleteAll} disabled={busy}>Delete all</button>
        </div>
        {/snippet}
    </Window>
{/if}

<style>
    .row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 16px;
        padding: 12px 0;
        border-top: 1px solid var(--line);
    }
    .note, .count, .message {
        margin: 0;
        color: var(--ink-soft);
        font-size: 13.5px;
        text-wrap: pretty;
    }
    /* The library is a second part: a line and more room than between two rows. */
    section {
        margin-top: 20px;
        padding-top: 16px;
        border-top: 1px solid var(--line);
    }
    h3 {
        margin: 0 0 2px;
        font: 700 14px/20px var(--sans);
    }
    .count strong {
        color: var(--ink);
        font-weight: 600;
    }
    .buttons {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
        margin-top: 12px;
    }
    button {
        padding: 6px 12px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
    }
    button:hover:not(:disabled) {
        background: var(--parchment-2);
    }
    .danger {
        border-color: var(--coral);
        color: var(--coral);
    }
    /* Beside the backup buttons the delete is plain text at the far end; its window asks before it deletes. */
    .buttons .danger {
        margin-left: auto;
        border-color: transparent;
    }
    .message {
        margin-top: 10px;
    }
    .message:empty {
        margin: 0;
    }
    @media (max-width: 760px) {
        .row { flex-wrap: wrap; }
    }
</style>
