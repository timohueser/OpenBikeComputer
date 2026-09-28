import { placeCategories, type PlaceCategory } from './poi-kinds';

const colors = {
    light: { panel: '#ffffff', ring: '#676443', ink: '#1c1b14', peak: '#5c5a2e' },
    dark: { panel: '#201f17', ring: '#aaa383', ink: '#f2efe3', peak: '#bdb47e' },
};
const ratio = 2;

/** Draws a style image on demand: `poi-<category>-<theme>` is a place glyph in a ring, `landmark-<theme>` an outline peak. */
export function mapIcon(id: string): { image: ImageData; pixelRatio: number } | null {
    const match = id.match(/^(?:poi-(\w+)|landmark)-(light|dark)$/);
    const category = match?.[1] as PlaceCategory | undefined;
    if (!match || (category && !placeCategories[category])) return null;
    const color = colors[match[2] as 'light' | 'dark'];
    const size = 20;
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = size * ratio;
    const context = canvas.getContext('2d')!;
    context.scale(ratio, ratio);
    context.lineCap = context.lineJoin = 'round';
    if (category) {
        context.beginPath();
        context.arc(size / 2, size / 2, size / 2 - 1, 0, Math.PI * 2);
        context.fillStyle = color.panel;
        context.fill();
        context.lineWidth = 1.2;
        context.strokeStyle = color.ring;
        context.stroke();
        glyph(context, placeCategories[category].icon, 4, 12, color.ink, 1.1);
    } else {
        const peak = new Path2D(placeCategories.peak.icon);
        context.save();
        context.translate(1, 1);
        context.scale(18 / 24, 18 / 24);
        context.fillStyle = color.panel;
        context.fill(peak);
        context.restore();
        glyph(context, placeCategories.peak.icon, 1, 18, color.peak, 1.6);
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
