/**
 * What each preset is *for*, in one line.
 *
 * Site-side on purpose. A preset config already carries a `_meta.description`, and that line
 * describes what the style *draws* ("roads by class, trails and cycleways, rail, water…") — a
 * packer fact, and the right thing for the config to own. What a card needs next to a picture is
 * the other sentence: which ride this is the map for. Putting that in the config would push
 * marketing copy through `obc-pack schema` and its pinning tests for no gain, so it lives here.
 *
 * Plain language. No wordplay, no exclamation, no adjective doing work a fact could do — the same
 * rule the device's own copy follows.
 *
 * A preset with no entry here is not a bug: {@link presetTagline} falls back to the config's own
 * description, so a preset dropped into `builder/presets/` gets a card with real words and a real
 * preview from one bake run. Add a line here when there is something better to say than what the
 * style table already says.
 */
const TAGLINES: Readonly<Record<string, string>> = {
    bikepacking: "Long tours on mixed surfaces — the map to ride a week off.",
};

/**
 * The one line a preset card shows under its name. Falls back to the preset's own description
 * where the site has nothing to add, so every card says something.
 */
export function presetTagline(presetId: string, description: string): string {
    return TAGLINES[presetId] ?? description;
}

/** Preset ids the site carries its own copy for — the guard's subject, and nothing else's. */
export function presetsWithCopy(): string[] {
    return Object.keys(TAGLINES);
}
