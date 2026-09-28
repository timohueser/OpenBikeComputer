export const ridingProfiles = {
    road: { label: 'Road bike', presets: ['Balanced', 'Quieter roads', 'Less climbing'] },
    gravel: { label: 'Gravel bike', presets: ['Balanced', 'Prefer unpaved', 'No singletrack'] },
    touring: { label: 'Touring bike', presets: ['Balanced', 'Quieter roads', 'Less climbing'] },
    mtb: { label: 'Mountain bike', presets: ['Balanced', 'Prefer trails', 'Easier terrain'] },
};
export type BikeType = keyof typeof ridingProfiles;
