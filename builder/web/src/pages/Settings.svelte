<script lang="ts">
    import { onMount } from 'svelte';
    import Card from '../lib/ui/Card.svelte';
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
        { key: 'maxHr', label: 'Maximum heart rate', unit: 'bpm', zones: heartRateZones, ramp: 'hr' },
        { key: 'ftp', label: 'Threshold power (FTP)', unit: 'W', zones: powerZones, ramp: 'power' },
    ] as const;

    let counts = $state<{ routes: number; rides: number } | null>(null);
    let message = $state('');
    let busy = $state(false);
    let confirming = $state(false);
    let failure = $state('');
    let file: HTMLInputElement;

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
    <div class="cards">
        <Card label="Rider">
            <div class="row">
                <strong>Units</strong>
                <Segmented label="Units" options={units} value={settings.units} onChange={value => updateSettings({ units: value })} />
            </div>
            <div class="row">
                <strong>Look</strong>
                <Segmented label="Look" options={looks} value={appearance.look} onChange={setLook} />
            </div>
            {#each values as { key, label, unit, zones, ramp } (key)}
                <RiderValue id={key} {label} {unit} value={settings[key]} range={RANGES[key]} {zones} {ramp} onSet={value => setValue(key, value)} />
            {/each}
            <p class="note">The zones come from these two values. A new ride from the OpenBikeComputer brings the values of the device, and they replace the values here. A ride from a file takes the values that are here. A ride keeps its values when you change them later.</p>
        </Card>
        <Card label="Your library">
            <p class="count">
                {#if counts}<strong>{plural(counts.routes, 'route')} and {plural(counts.rides, 'ride')}</strong> are saved in this browser, on this computer only.{/if}
            </p>
            <div class="buttons">
                <button type="button" onclick={save} disabled={busy || !counts || counts.routes + counts.rides === 0}>Save a backup file</button>
                <button type="button" onclick={() => file.click()} disabled={busy}>Load a backup file</button>
                <input bind:this={file} type="file" accept=".zip,application/zip" hidden onchange={event => load(event.currentTarget.files?.[0])} />
                <button type="button" class="danger" onclick={() => confirming = true} disabled={busy}>Delete all</button>
            </div>
            <p class="message" role="status">{message}</p>
        </Card>
    </div>
</Window>

{#if confirming}
    <Window title="Delete all?" onClose={() => { confirming = false; failure = ''; }}>
        {#snippet children(close)}
        <p>Your routes and rides leave this browser. The device keeps its own, and its rides come in again at the next connection. Close other tabs of this app first.</p>
        {#if failure}<p class="message" role="alert">{failure}</p>{/if}
        <div class="buttons end">
            <button type="button" onclick={close}>Cancel</button>
            <button type="button" class="danger" onclick={deleteAll} disabled={busy}>Delete all</button>
        </div>
        {/snippet}
    </Window>
{/if}

<style>
    .cards {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 16px;
        align-items: start;
    }
    .row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 16px;
        padding: 9px 0;
        border-top: 1px solid var(--line);
    }
    p {
        margin: 4px 0 0;
    }
    .row p, .note {
        color: var(--ink-soft);
        font-size: 12.5px;
    }
    .note {
        margin: 0;
        padding-top: 12px;
        border-top: 1px solid var(--line);
    }
    .count {
        margin: 0 0 12px;
    }
    .buttons {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
    }
    .end {
        justify-content: flex-end;
        margin-top: 16px;
    }
    button {
        min-height: 32px;
        padding: 5px 10px;
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
    .buttons:not(.end) .danger {
        margin-left: auto;
    }
    .message {
        margin-top: 10px;
    }
    .message:empty {
        margin: 0;
    }
    @media (max-width: 760px) {
        .cards { grid-template-columns: minmax(0, 1fr); }
        .row { flex-wrap: wrap; }
    }
</style>
