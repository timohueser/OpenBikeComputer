/** `kmh` times a manual leg: the route engine's default pace for the activity on flat ground (`specs/route-api.md`, `cycling_pace` in route-engine `model.rs`). */
export const ridingProfiles = {
    road: { icon: 'road', label: 'Road bike', kmh: 19 },
    gravel: { icon: 'gravel', label: 'Gravel bike', kmh: 19 },
    touring: { icon: 'touring', label: 'Touring bike', kmh: 19 },
    mtb: { icon: 'mtb', label: 'Mountain bike', kmh: 15.6 },
    hiking: { icon: 'hiking', label: 'Hiking', kmh: 4.5 },
};
export type BikeType = keyof typeof ridingProfiles;

/** The preset title of each profile ID suffix: `touring/shorter` is the Shorter preset of the touring bike. The routing
 * region says which profiles it serves (`GET /v1/region`). */
const presetTitles: Record<string, string> = { '': 'Balanced', shorter: 'Shorter', 'less-climbing': 'Less climbing' };
/** Every preset title that a plan can keep. */
export const presetNames = Object.values(presetTitles);

export function presetName(profile: string): string {
    return presetTitles[profile.split('/')[1] ?? ''] ?? 'Balanced';
}
export function presetSuffix(name = 'Balanced'): string {
    return Object.entries(presetTitles).find(([, title]) => title === name)?.[0] ?? '';
}

/** The preset titles of `bike` that the region's `profiles` serve; all of them while the region is unknown. */
export function servedPresets(bike: BikeType, profiles: string[] | undefined): string[] {
    return Object.entries(presetTitles).filter(([suffix]) => !profiles || profiles.includes(suffix ? `${bike}/${suffix}` : bike))
        .map(([, title]) => title);
}
