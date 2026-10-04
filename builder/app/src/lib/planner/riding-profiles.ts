/** `kmh` times a manual leg: the route engine's default pace for the activity on flat ground (`specs/route-api.md`, `cycling_pace` in route-engine `model.rs`). */
export const ridingProfiles = {
    road: { icon: 'road', label: 'Road bike', presets: ['Balanced', 'Shorter', 'Less climbing'], kmh: 19 },
    gravel: { icon: 'gravel', label: 'Gravel bike', presets: ['Balanced', 'Shorter', 'Less climbing'], kmh: 19 },
    touring: { icon: 'touring', label: 'Touring bike', presets: ['Balanced', 'Shorter', 'Less climbing'], kmh: 19 },
    mtb: { icon: 'mtb', label: 'Mountain bike', presets: ['Balanced', 'Shorter', 'Less climbing'], kmh: 15.6 },
    hiking: { icon: 'hiking', label: 'Hiking', presets: ['Balanced', 'Shorter', 'Less climbing'], kmh: 4.5 },
};
export type BikeType = keyof typeof ridingProfiles;

const presetNames: Record<string, string> = {
    shorter: 'Shorter', smoother: 'Smoother', 'less-climbing': 'Less climbing', quieter: 'Quieter',
};
export function presetName(profile: string): string {
    return presetNames[profile.split('/')[1]] ?? 'Balanced';
}
export function presetSuffix(name = 'Balanced'): string {
    return Object.entries(presetNames).find(([, label]) => label === name)?.[0] ?? '';
}
