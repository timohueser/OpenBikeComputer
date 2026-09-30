export const ridingProfiles = {
    road: { label: 'Road bike', presets: ['Balanced', 'Less climbing'] },
    gravel: { label: 'Gravel bike', presets: ['Balanced', 'Less climbing'] },
    touring: { label: 'Touring bike', presets: ['Balanced', 'Less climbing'] },
    mtb: { label: 'Mountain bike', presets: ['Balanced', 'Less climbing'] },
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
