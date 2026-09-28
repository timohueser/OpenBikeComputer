export const ridingProfiles = {
    road: { label: 'Road bike', presets: ['Balanced', 'Shorter', 'Smoother', 'Less climbing'] },
    gravel: { label: 'Gravel bike', presets: ['Balanced', 'Shorter', 'Smoother', 'Less climbing'] },
    touring: { label: 'Touring bike', presets: ['Balanced', 'Shorter', 'Smoother', 'Less climbing'] },
    mtb: { label: 'Mountain bike', presets: ['Balanced', 'Shorter', 'Smoother', 'Less climbing'] },
};
export type BikeType = keyof typeof ridingProfiles;
