<script lang="ts">
    // The routing-profile editor: one card per bike type (the climb weight + a
    // per-class multiplier grid for highway and surface classes) and per-type
    // "Reset to defaults". The four types and their order are fixed. Every bound and
    // every class name is read from the served schema (readProfileSchema) — the
    // shipped defaults come from the packer, not a frontend copy.
    import type { ClassGroup } from "../../lib/config/profiles";
    import {
        cellValue,
        classNames,
        clearCell,
        clearClimbWeight,
        displayProfiles,
        ensureRouting,
        hasClimbWeight,
        isExplicit,
        profileClimbWeight,
        profileDefault,
        readProfileSchema,
        resetProfile,
        setCell,
        setClimbWeight,
        setProfileDefault,
    } from "../../lib/config/profiles";
    import type { Multiplier, SchemaEnvelope } from "../../lib/config/model";
    import { working } from "../../lib/config/storage.svelte";
    import { confirmAction } from "../../lib/ui/confirm.svelte";
    import ClimbWeightCell from "./ClimbWeightCell.svelte";
    import MultiplierCell from "./MultiplierCell.svelte";

    let { schema }: { schema: SchemaEnvelope | null } = $props();

    const env = $derived(working.envelope!);
    const ps = $derived(readProfileSchema(schema));
    // The list shown: the config's own profiles, or the shipped defaults until
    // the user edits (an untouched CLI config keeps no `routing` section).
    const profiles = $derived(ps ? displayProfiles(env.config, ps) : []);
    const groups: ClassGroup[] = ["highway", "surface"];

    let hint = $state<string | null>(null);

    function touch() {
        working.markModified();
    }

    function editDefault(i: number, v: Multiplier) {
        if (!ps) return;
        setProfileDefault(ensureRouting(env.config, ps).profiles[i], v);
        touch();
    }

    function editClimb(i: number, v: number) {
        if (!ps) return;
        setClimbWeight(ensureRouting(env.config, ps).profiles[i], v);
        touch();
    }

    function revertClimb(i: number) {
        if (!ps) return;
        clearClimbWeight(ensureRouting(env.config, ps).profiles[i]);
        touch();
    }

    function editCell(i: number, group: ClassGroup, cls: string, v: Multiplier) {
        if (!ps) return;
        setCell(ensureRouting(env.config, ps).profiles[i], group, cls, v);
        touch();
    }

    function revertCell(i: number, group: ClassGroup, cls: string) {
        if (!ps) return;
        clearCell(ensureRouting(env.config, ps).profiles[i], group, cls);
        touch();
    }

    async function reset(i: number) {
        if (!ps) return;
        const name = profiles[i]?.name ?? "this profile";
        const ok = await confirmAction({
            title: `Reset “${name}” to its shipped defaults?`,
            body: "Your tweaks to it are discarded. The other profiles are untouched.",
            confirmLabel: "Reset",
            destructive: true,
        });
        // `ps` is re-read after the await: the schema could have arrived, or gone, while the
        // dialog was up.
        if (!ok || !ps) return;
        hint = null;
        resetProfile(env.config, i, ps);
        touch();
    }
</script>

{#if !ps}
    <div class="card">
        <p class="muted">
            The obc-pack build on this machine doesn't expose the routing schema, so the profile
            editor is unavailable. Rebuild obc-pack (OBCM v9+) to edit bike profiles.
        </p>
    </div>
{:else}
    <div class="intro">
        <p class="muted small">
            Bike profiles weight the nav graph so the router prefers some way and surface types over
            others. Each multiplier is ≥ 1.0 (1.0 = neutral, higher = avoid); unlisted classes use the
            profile's default. The <b>climb weight</b> ({ps.climbMin}–{ps.climbMax}) is separate: flat
            metres charged per metre of ascent, added to the cost rather than scaling it, so
            {ps.climbMin} is climb-blind and only maps packed with terrain feel it. The device picks one
            of the four bike types. Their names and order are fixed; only the weights change.
        </p>
    </div>

    {#if hint}
        <p class="hint small">{hint}</p>
    {/if}

    {#each profiles as profile, i (i)}
        <div class="card profile">
            <div class="phead">
                <span class="pname">{profile.name}</span>
                <div class="pdefault">
                    <span class="small muted">other classes</span>
                    <MultiplierCell
                        value={profileDefault(profile, ps)}
                        explicit={true}
                        min={ps.multiplierMin}
                        label={`${profile.name} default`}
                        onset={(v) => editDefault(i, v)}
                        onclear={() => editDefault(i, ps.defaultMultiplier)}
                        onhint={(h) => (hint = h)}
                    />
                </div>
                <div class="pdefault">
                    <span class="small muted" title="Flat metres charged per metre of ascent (§8.6)">
                        climb weight
                    </span>
                    <ClimbWeightCell
                        value={profileClimbWeight(profile, ps)}
                        stated={hasClimbWeight(profile)}
                        min={ps.climbMin}
                        max={ps.climbMax}
                        label={`${profile.name} climb weight`}
                        onset={(v) => editClimb(i, v)}
                        onclear={() => revertClimb(i)}
                        onhint={(h) => (hint = h)}
                    />
                </div>
                <span class="spacer"></span>
                <button type="button" class="btn ghost small" onclick={() => reset(i)}>
                    Reset to defaults
                </button>
            </div>

            {#each groups as group (group)}
                <div class="group">
                    <span class="glabel small">{group}</span>
                    <div class="grid">
                        {#each classNames(ps, group) as cls (cls)}
                            <div class="pair">
                                <span class="clabel mono" title={cls}>{cls}</span>
                                <MultiplierCell
                                    value={cellValue(profile, group, cls, ps)}
                                    explicit={isExplicit(profile, group, cls)}
                                    min={ps.multiplierMin}
                                    label={`${profile.name} · ${group} · ${cls}`}
                                    onset={(v) => editCell(i, group, cls, v)}
                                    onclear={() => revertCell(i, group, cls)}
                                    onhint={(h) => (hint = h)}
                                />
                            </div>
                        {/each}
                    </div>
                </div>
            {/each}
        </div>
    {/each}
{/if}

<style>
    .intro {
        display: flex;
        align-items: flex-start;
        justify-content: space-between;
        gap: 18px;
        margin-bottom: 10px;
    }

    .intro p {
        max-width: 68ch;
    }

    .hint {
        color: var(--coral);
        margin: 0 0 12px;
        max-width: 78ch;
    }

    .profile {
        margin-bottom: 14px;
        padding: 14px;
    }

    .phead {
        display: flex;
        align-items: center;
        gap: 14px;
        flex-wrap: wrap;
        margin-bottom: 12px;
    }

    .pname {
        font-size: 15px;
        font-weight: 600;
        padding: 4px 8px;
        width: 160px;
    }

    .pdefault {
        display: flex;
        align-items: center;
        gap: 6px;
    }

    .spacer {
        flex: 1;
    }

    .group + .group {
        margin-top: 10px;
        padding-top: 10px;
        border-top: 1px solid var(--line);
    }

    .glabel {
        display: block;
        color: var(--ink-faint);
        text-transform: uppercase;
        letter-spacing: 0.4px;
        margin-bottom: 6px;
    }

    .grid {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
        gap: 5px 16px;
    }

    .pair {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
    }

    .clabel {
        font-size: 12px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
</style>
