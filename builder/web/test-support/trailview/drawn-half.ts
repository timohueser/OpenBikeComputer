// The ways that the engine and the bake must draw alike: each road class on each surface, as tags, with the kind of
// feature that carries them. `drawn-half.json` holds the engine's half-width of each, which the bake checks.
import CLASS_RULES from '../../../../data/trailview/roads.json' with { type: 'json' };

const RAILS = ['rail', 'light_rail', 'narrow_gauge', 'rack', 'funicular'];
const PATHS = ['path', 'footway', 'cycleway', 'bridleway', 'steps', 'pedestrian', 'via_ferrata'];
const SURFACE_TAGS = ['', 'asphalt', 'concrete', 'paving_stones', 'sett', 'gravel', 'compacted', 'dirt', 'ground', 'grass', 'grass_paver'];

/** Each case: the kind of feature and its tags. */
export function drawnHalfCases(): { kind: string; tags: Record<string, string> }[] {
    const out: { kind: string; tags: Record<string, string> }[] = [];
    for (const name of Object.keys(CLASS_RULES)) {
        if (RAILS.includes(name)) {
            const tags: Record<string, string> = name === 'rack' ? { railway: 'rail', rack: 'yes' } : { railway: name };
            out.push({ kind: 'railway', tags }, { kind: 'railway', tags: { ...tags, gauge: '1000' } });
            continue;
        }
        const kind = PATHS.includes(name) ? 'path' : 'road';
        for (const surface of SURFACE_TAGS) out.push({ kind, tags: surface ? { highway: name, surface } : { highway: name } });
        out.push({ kind, tags: { highway: name, oneway: 'yes' } }, { kind, tags: { highway: name, width: '5' } });
    }
    for (const tracktype of ['grade1', 'grade2', 'grade3', 'grade4']) out.push({ kind: 'road', tags: { highway: 'track', tracktype } });
    out.push({ kind: 'road', tags: { highway: 'unknown_class' } }, { kind: 'road', tags: { highway: 'motorway_link', oneway: 'no' } }, { kind: 'road', tags: { highway: 'primary', junction: 'roundabout' } });
    return out;
}
