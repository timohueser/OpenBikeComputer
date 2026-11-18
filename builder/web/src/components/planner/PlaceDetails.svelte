<script lang="ts">
    import Icon from '../../lib/ui/Icon.svelte';
    import OpeningHours from './OpeningHours.svelte';
    import PlaceContent from './PlaceContent.svelte';
    import { kindLabel } from '../../lib/planner/search/presentation';
    import { websiteLink, phoneNumbers, phoneLink } from '../../lib/planner/contact-links';
    import { placeCategories } from '../../lib/planner/poi-kinds';
    import type { Place } from '../../lib/planner/editor';

    /** The details of a place in a map popup: its symbol and name when `titled`, its kind, its article, note, contacts
     * and opening hours. */
    let { place, titled = true, aside, error }: {
        place: Place;
        titled?: boolean;
        /** Added to the kind line. */
        aside?: string;
        error?: string;
    } = $props();

    const website = $derived(websiteLink(place.website));
    const phones = $derived(phoneNumbers(place.phone));
</script>

{#snippet kind()}
    <p class="kind">{[place.placeKind ? kindLabel(place.placeKind) : placeCategories[place.category].label, place.locality, aside].filter(Boolean).join(' · ')}</p>
{/snippet}
{#if titled}
    <div class="place-heading">
        <span class="place-symbol"><Icon path={placeCategories[place.category].icon} size={21} /></span>
        <div><h2>{place.label}</h2>{@render kind()}</div>
    </div>
{:else}
    {@render kind()}
{/if}
{#if place.content}<PlaceContent content={place.content} name={place.label} />{/if}
{#if place.description && place.description !== placeCategories[place.category].label}<p class="place-note">{place.description}</p>{/if}
{#if website || place.website || phones.length}
    <div class="contacts">
        {#if website}<a href={website} target="_blank" rel="noopener noreferrer">Website</a>
        {:else if place.website}<span>{place.website}</span>{/if}
        {#each phones as phone}
            {@const link = phoneLink(phone)}
            {#if link}<a href={link}>{phone}</a>{:else}<span>{phone}</span>{/if}
        {/each}
    </div>
{/if}
{#if error}<p class="hint" role="status">{error}</p>{/if}
{#if place.openingHours || ['shop','food','pharmacy','hotel','bike'].includes(place.category)}<OpeningHours value={place.openingHours} />{/if}

<style>
    .place-heading { display: flex; gap: 12px; align-items: flex-start; margin-right: 16px; }
    .place-heading h2 { margin: 0 12px 5px 0; font: 600 17px/1.3 var(--sans); overflow-wrap: anywhere; }
    .place-symbol { display: grid; place-items: center; width: 40px; height: 40px; flex: none; border-radius: 50%; color: var(--query-place); background: color-mix(in srgb, var(--query-place) 10%, var(--panel)); }
    .kind { margin: 0; color: var(--ink-soft); font-size: 13px; line-height: 1.45; }
    .place-note { margin: 10px 0; color: var(--ink); line-height: 1.5; white-space: pre-wrap; overflow-wrap: anywhere; }
    .contacts { display: flex; flex-wrap: wrap; gap: 4px 16px; margin: 12px 0; overflow-wrap: anywhere; }
    .contacts a { color: var(--ink); text-underline-offset: 3px; padding: 4px 0; }
    .contacts a:hover { color: var(--query-place); }
    .contacts a:focus-visible { outline: 2px solid var(--query-place); outline-offset: 3px; border-radius: 2px; }
    .hint { margin: 4px 0 0; color: var(--ink-soft); }
</style>
