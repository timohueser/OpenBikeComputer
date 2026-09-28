import { anchorProgress, type Coordinate, type Place } from './editor';

// Fictional passes and summits a few kilometres off the example route.
export const landmarks: Place[] = ([
    { label: 'Mont Chauvelin', elevation: 812, coordinate: [7.262, 47.535] },
    { label: 'Col de la Roche Fendue', elevation: 1120, coordinate: [6.955, 47.448] },
    { label: 'Signal de Frênois', elevation: 947, coordinate: [6.585, 47.318] },
    { label: 'Col des Trois Hêtres', elevation: 684, coordinate: [6.205, 47.205] },
] as { label: string; elevation: number; coordinate: Coordinate }[]).map((landmark, index) => ({
    id: `landmark-${index}`, kind: 'place', category: 'peak', label: landmark.label, coordinate: landmark.coordinate,
    progress: anchorProgress(landmark.coordinate), description: `${landmark.elevation.toLocaleString('en')} m`,
}));
