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
const presets: Record<string, { title: string; activity?: BikeType; description?: string }> = {
    '': { title: 'Balanced' },
    shorter: { title: 'Shorter' },
    'less-climbing': { title: 'Less climbing' },
    technical: { title: 'Technical (S3)', activity: 'mtb', description: 'Trails up to S3, with steep slopes and large obstacles. Advanced riding skills required.' },
    alpine: { title: 'Alpine (T4)', activity: 'hiking', description: 'Alpine hiking up to T4, including exposed terrain. Sure-footedness and alpine experience required.' },
};

export function presetName(profile: string): string {
    const [activity, suffix = ''] = profile.split('/');
    const preset = presets[suffix];
    return preset && (!preset.activity || preset.activity === activity) ? preset.title : 'Balanced';
}
export function presetSuffix(name = 'Balanced'): string {
    return Object.entries(presets).find(([, preset]) => preset.title === name)?.[0] ?? '';
}

export function presetDescription(name: string): string | undefined {
    return Object.values(presets).find(preset => preset.title === name)?.description;
}

export function presetAppliesTo(name: string, bike: BikeType): boolean {
    return Object.values(presets).some(preset => preset.title === name && (!preset.activity || preset.activity === bike));
}

/** Advanced terrain presets require an advertised profile; an unknown region offers the three ordinary presets. */
export function servedPresets(bike: BikeType, profiles: string[] | undefined): string[] {
    return Object.entries(presets).filter(([suffix, preset]) => (!preset.activity || preset.activity === bike)
        && (profiles ? profiles.includes(suffix ? `${bike}/${suffix}` : bike) : !preset.activity))
        .map(([, preset]) => preset.title);
}
