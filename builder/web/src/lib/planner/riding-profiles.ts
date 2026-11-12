/** `kmh` times a manual leg: the route engine's default pace for the activity on flat ground (`specs/route-api.md`, `cycling_pace` in route-engine `model.rs`). */
export const ridingProfiles = {
    road: { icon: 'road', label: 'Road bike', kmh: 19 },
    gravel: { icon: 'gravel', label: 'Gravel bike', kmh: 19 },
    touring: { icon: 'touring', label: 'Touring bike', kmh: 19 },
    mtb: { icon: 'mtb', label: 'Mountain bike', kmh: 15.6 },
    hiking: { icon: 'hiking', label: 'Hiking', kmh: 4.5 },
};
export type BikeType = keyof typeof ridingProfiles;

/** The factor of each pace on the riding time of the route engine. Moderate is the pace of the engine: a bike that rides
 * at 19 km/h on the flat at Moderate rides at 27 km/h at Fast. */
export const paces = {
    easy: { label: 'Easy', factor: 19 / 15 },
    moderate: { label: 'Moderate', factor: 1 },
    sporty: { label: 'Sporty', factor: 19 / 23 },
    fast: { label: 'Fast', factor: 19 / 27 },
};
export type Pace = keyof typeof paces;

/** The pace of a plan: Moderate when the plan has none or one that this reader does not know (`specs/planner-plan.md`). */
export function planPace(trip: { pace?: string }): Pace {
    return trip.pace !== undefined && Object.hasOwn(paces, trip.pace) ? trip.pace as Pace : 'moderate';
}

/** The preset title of each profile ID suffix: `touring/less-climbing` is the Less climbing preset of the touring bike. The
 * routing region says which profiles it serves (`GET /v1/region`). Technical (MTB) and Alpine (hiking) allow harder trails. */
const presetTitles: Record<string, string> = { '': 'Balanced', 'less-climbing': 'Less climbing', technical: 'Technical', alpine: 'Alpine' };
/** The presets of each activity, Balanced first (`specs/planner-plan.md`). */
const activityPresets: Record<BikeType, string[]> = {
    road: ['Balanced', 'Less climbing'],
    gravel: ['Balanced', 'Less climbing'],
    touring: ['Balanced', 'Less climbing'],
    mtb: ['Balanced', 'Technical'],
    hiking: ['Balanced', 'Alpine'],
};
const skillPresets = ['Technical', 'Alpine'];

export function presetName(profile: string): string {
    return presetTitles[profile.split('/')[1] ?? ''] ?? 'Balanced';
}

/** The preset that a plan routes with: its own preset when its activity has it, else Balanced. A saved plan can hold a
 * preset that its activity does not have. */
export function planPreset(bike: BikeType = 'touring', preset = 'Balanced'): string {
    return activityPresets[bike].includes(preset) ? preset : 'Balanced';
}

/** The profile ID that a plan with `bike` and `preset` routes with. */
export function presetProfile(bike: BikeType = 'touring', preset?: string): string {
    const title = planPreset(bike, preset);
    const suffix = Object.entries(presetTitles).find(([, name]) => name === title)![0];
    return suffix ? `${bike}/${suffix}` : bike;
}

/** The presets of `bike` that the region's `profiles` serve; while the region is unknown, its presets without a skill preset. */
export function servedPresets(bike: BikeType, profiles: string[] | undefined): string[] {
    return activityPresets[bike].filter(title => profiles ? profiles.includes(presetProfile(bike, title)) : !skillPresets.includes(title));
}
