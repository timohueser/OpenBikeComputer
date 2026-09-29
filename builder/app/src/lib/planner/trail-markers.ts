/** Common geometric OSM trail blazes. Unsupported symbols stay unrendered. */
const colors: Record<string, string> = {
    white: '#fff', black: '#171717', gray: '#777', red: '#dc2525', blue: '#1968bf', yellow: '#f4d32d',
    green: '#258140', orange: '#ec8526', brown: '#87532b', purple: '#87449b',
};
type Mark = { color: string; shape: string };
export interface TrailMarker { background: Mark | null; foreground: Mark[]; text: string; textColor: string }
const paths: Record<string, string> = {
    diamond: 'M3 12 12 6 21 12 12 18Z', bar: 'M3 10H21V14H3Z', stripe: 'M10 3H14V21H10Z',
    cross: 'M10 4H14V10H20V14H14V20H10V14H4V10H10Z',
    x: 'M5 3 12 10 19 3 21 5 14 12 21 19 19 21 12 14 5 21 3 19 10 12 3 5Z',
    slash: 'M3 18 18 3 21 6 6 21Z', backslash: 'M3 6 6 3 21 18 18 21Z',
    triangle: 'M12 4 21 20H3Z', triangle_turned: 'M3 4H21L12 20Z',
    rectangle: 'M3 7H21V17H3Z', square: 'M5 5H19V19H5Z',
};

export function trailMarker(value: unknown): TrailMarker | null {
    if (typeof value !== 'string' || value.length > 128) return null;
    const parts = value.split(':');
    if (parts.length < 2 || parts.length > 6) return null;
    const [, background, ...rest] = parts;
    let text = '', textColor = colors.black;
    if (rest.length >= 3) {
        const color = rest.pop()!;
        text = rest.pop()!;
        if (!colors[color] || [...text].length > 4) return null;
        textColor = colors[color];
    }
    const parse = (s: string, bg: boolean): Mark | null => {
        if (s === 'white_red_diamond' && !bg) return { color: colors.white, shape: s };
        const [color, ...suffix] = s.split('_');
        const shape = suffix.join('_') || (bg ? 'square' : '');
        const base = shape.replace(/_line$/, '');
        if (!colors[color] || !(bg ? ['square','round','circle','frame'].includes(shape)
            : base in paths || ['dot','circle','frame'].includes(shape))) return null;
        return { color: colors[color], shape };
    };
    const bg = background ? parse(background, true) : null;
    if (background && !bg) return null;
    const foreground: Mark[] = [];
    for (const part of rest.filter(Boolean)) {
        const mark = parse(part, false);
        if (!mark) return null;
        foreground.push(mark);
    }
    return bg || foreground.length || text ? { background: bg, foreground, text, textColor } : null;
}

export function trailImage(value: string): ImageData | null {
    const marker = trailMarker(value);
    if (!marker) return null;
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = 48;
    const c = canvas.getContext('2d')!;
    c.scale(2, 2);
    c.lineWidth = 1.5;
    if (marker.background) {
        const { color, shape } = marker.background;
        c.fillStyle = ['circle','frame'].includes(shape) ? '#fff' : color;
        c.strokeStyle = color;
        c.beginPath();
        if (['circle','round'].includes(shape)) c.arc(12,12,10,0,2*Math.PI);
        else c.rect(2,2,20,20);
        c.fill();
        c.stroke();
    }
    for (const {color,shape} of marker.foreground) {
        c.fillStyle = c.strokeStyle = color;
        if (shape === 'white_red_diamond') {
            c.fill(new Path2D(paths.diamond));
            c.fillStyle = colors.red;
            c.fill(new Path2D('M12 6 21 12 12 18Z'));
        } else if (shape === 'circle' || shape === 'dot') {
            c.beginPath(); c.arc(12,12,6,0,2*Math.PI);
            if (shape === 'circle') c.stroke(); else c.fill();
        } else if (shape === 'frame') {
            c.strokeRect(4,4,16,16);
        } else {
            const p = new Path2D(paths[shape.replace(/_line$/, '')]);
            if (shape.endsWith('_line')) c.stroke(p); else c.fill(p);
        }
    }
    if (marker.text) {
        c.font = 'bold 10px sans-serif'; c.textAlign = 'center'; c.textBaseline = 'middle';
        c.fillStyle = marker.textColor; c.fillText(marker.text,12,12,18);
    }
    return c.getImageData(0,0,48,48);
}
