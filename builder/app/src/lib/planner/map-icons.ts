import { placeCategories, type PlaceCategory } from './poi-kinds';
import { trailImage } from './trail-markers';

const colors = {
    light: { panel: '#ffffff', ring: '#676443', ink: '#1c1b14' },
    dark: { panel: '#201f17', ring: '#aaa383', ink: '#f2efe3' },
};
const ratio = 2;

/** Draws place and access symbols on demand for the active theme. */
export function mapIcon(id: string): { image: ImageData; pixelRatio: number } | null {
    if (id.startsWith('trail:')) {
        const image = trailImage(id.slice(6));
        return image ? { image, pixelRatio: ratio } : null;
    }
    const match = id.match(/^(?:poi-(\w+)|(access|conditional|push|no-bikes))-(light|dark)$/);
    const category = match?.[1] as PlaceCategory | undefined;
    if (!match || (category && !placeCategories[category])) return null;
    const color = colors[match[3] as 'light' | 'dark'];
    const size = 20;
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = size * ratio;
    const context = canvas.getContext('2d')!;
    context.scale(ratio, ratio);
    context.lineCap = context.lineJoin = 'round';
    if (match[2] === 'push') {
        glyph(context, 'M13 3a2 2 0 1 0 0 4 2 2 0 0 0 0-4ZM9 22l3-7 4 7M12 15l-1-6 4 3 4 1M11 9l-4 4-1 4', 0, 20, color.ring, 1.6);
    } else if (match[2] === 'no-bikes') {
        glyph(context, 'M6 13a4 4 0 1 0 0 8 4 4 0 0 0 0-8ZM18 13a4 4 0 1 0 0 8 4 4 0 0 0 0-8ZM6 17l5-8 7 8-3-12h3M9 9h5M3 3l18 18', 0, 20, color.ring, 1.3);
    } else if (match[2] === 'access' || match[2] === 'conditional') {
        context.beginPath();
        context.arc(10, 10, 8, 0, Math.PI * 2);
        context.fillStyle = color.panel;
        context.fill();
        context.lineWidth = 1.4;
        context.strokeStyle = color.ring;
        context.stroke();
        glyph(context, match[2] === 'access' ? 'M6 12h12' : 'M9 9a3 3 0 1 1 4.5 2.6c-1.5.9-1.5 1.4-1.5 2.4m0 3v.1', 1, 18, color.ring, 1.8);
    } else if (category) {
        context.beginPath();
        context.arc(size / 2, size / 2, size / 2 - 1, 0, Math.PI * 2);
        context.fillStyle = color.panel;
        context.fill();
        context.lineWidth = 1.2;
        context.strokeStyle = color.ring;
        context.stroke();
        glyph(context, placeCategories[category].icon, 4, 12, color.ink, 1.1);
    }
    return { image: context.getImageData(0, 0, canvas.width, canvas.height), pixelRatio: ratio };
}

function glyph(context: CanvasRenderingContext2D, path: string, offset: number, size: number, color: string, width: number) {
    context.save();
    context.translate(offset, offset);
    context.scale(size / 24, size / 24);
    context.lineWidth = width * 24 / size;
    context.strokeStyle = color;
    context.stroke(new Path2D(path));
    context.restore();
}
