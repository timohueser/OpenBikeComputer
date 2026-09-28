// Shared example heights for the profile and its displayed ascent figures.
export const profileHeights = Array.from({ length: 121 }, (_, i) => 430 + Math.sin(i * .047) * 90 + Math.sin(i * .21) ** 2 * 120 + Math.sin(i * .073) ** 4 * 110);

export function profileAscent(from = 0, to = 1): number {
    let ascent = 0;
    for (let i = Math.max(1, Math.ceil(from * 120)); i <= Math.min(120, Math.floor(to * 120)); i++) {
        ascent += Math.max(0, profileHeights[i] - profileHeights[i - 1]);
    }
    return Math.round(ascent / 10) * 10;
}

export function profileHeightAt(progress: number): number {
    const x = Math.max(0, Math.min(1, progress)) * 120;
    const i = Math.min(119, Math.floor(x));
    return profileHeights[i] + (profileHeights[i + 1] - profileHeights[i]) * (x - i);
}
