export const ridingProfiles = {
    road: { icon: 'road', label: 'Road bike', presets: ['Balanced', 'Shorter', 'Less climbing'] },
    gravel: { icon: 'gravel', label: 'Gravel bike', presets: ['Balanced', 'Shorter', 'Less climbing'] },
    touring: { icon: 'touring', label: 'Touring bike', presets: ['Balanced', 'Shorter', 'Less climbing'] },
    mtb: { icon: 'mtb', label: 'Mountain bike', presets: ['Balanced', 'Shorter', 'Less climbing'] },
    hiking: { icon: 'hiking', label: 'Hiking', presets: ['Balanced', 'Shorter', 'Less climbing'] },
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
