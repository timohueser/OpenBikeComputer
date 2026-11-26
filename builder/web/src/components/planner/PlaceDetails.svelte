<script lang="ts">
    import type { Snippet } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import OpeningHours from './OpeningHours.svelte';
    import PlaceContent from './PlaceContent.svelte';
    import { kindLabel } from '../../lib/planner/search/presentation';
    import { websiteLabel, websiteLink, phoneNumbers, phoneLink } from '../../lib/planner/contact-links';
    import { placeCategories } from '../../lib/planner/poi-kinds';
    import type { Place } from '../../lib/planner/editor';

    /** The details of a place in a map popup: its symbol, its name or the `title` in its place, its kind, its article,
     * note, contacts and opening hours. */
    let { place, title, aside, error }: {
        place: Place;
        title?: Snippet;
        /** Added to the kind line. */
        aside?: string;
        error?: string;
    } = $props();

    const website = $derived(websiteLink(place.website));
    const phones = $derived(phoneNumbers(place.phone));
    const status = $derived(place.hoursStatus);
</script>

<header>
    <span class="glyph place"><Icon path={placeCategories[place.category].icon} size={16} /></span>
    <div>
        {#if title}{@render title()}{:else}<h2>{place.label}</h2>{/if}
        <p class="meta kind">{[place.placeKind ? kindLabel(place.placeKind) : placeCategories[place.category].label, place.locality, aside].filter(Boolean).join(' · ')}</p>
    </div>
</header>
{#if place.description && place.description !== placeCategories[place.category].label}<p class="place-note">{place.description}</p>{/if}
{#if place.openingHours || website || place.website || phones.length || ['shop','food','pharmacy','hotel','bike'].includes(place.category)}
    <ul class="facts">
        {#if place.openingHours}
            <!-- The state now is the answer, and the week of hours opens under it. With no state, the week shows. -->
            <li><details open={!status || status.state === 'unknown'}>
                <summary class="fact"><Icon name="clock" size={15} /><span>{#if status?.state === 'open'}<b class="open">Open</b>{#if status.closesAt}{' · closes '}{status.closesAt}{/if}{:else if status?.state === 'closed'}<b>Closed</b>{:else}Opening hours{/if}</span><Icon name="down" size={15} /></summary>
                <OpeningHours value={place.openingHours} heading={false} />
            </details></li>
        {:else if ['shop','food','pharmacy','hotel','bike'].includes(place.category)}
            <li class="fact soft"><Icon name="clock" size={15} /><span>No opening hours listed</span></li>
        {/if}
        {#if website}<li class="fact"><Icon name="globe" size={15} /><a href={website} target="_blank" rel="noopener noreferrer">{websiteLabel(website)}</a></li>
        {:else if place.website}<li class="fact"><Icon name="globe" size={15} /><span>{place.website}</span></li>{/if}
        {#each phones as phone}
            {@const link = phoneLink(phone)}
            <li class="fact"><Icon name="phone" size={15} />{#if link}<a href={link}>{phone}</a>{:else}<span>{phone}</span>{/if}</li>
        {/each}
    </ul>
{/if}
{#if place.content}<PlaceContent content={place.content} name={place.label} />{/if}
{#if error}<p class="meta" role="status">{error}</p>{/if}

<style>
    /* A kind of the search data is in small letters. */
    .kind::first-letter { text-transform: uppercase; }
    .place-note { line-height: 1.5; white-space: pre-wrap; overflow-wrap: anywhere; }
    summary { list-style: none; cursor: pointer; }
    summary::-webkit-details-marker { display: none; }
    summary :global(svg:last-child) { margin-left: auto; transition: transform 140ms ease-out; }
    details[open] summary :global(svg:last-child) { transform: rotate(180deg); }
    .open { color: var(--water); font-weight: 600; }
    .soft { color: var(--ink-soft); }
    @media (prefers-reduced-motion: reduce) { summary :global(svg:last-child) { transition: none; } }
</style>
