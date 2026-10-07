// Light colours carry white text and dark colours carry the dark panel colour at 4.5:1 or more.
const light = ['#cc2a93', '#2f6fb5', '#2e7d32', '#5b2fb0'];
const dark = ['#e45cb5', '#6fa8e8', '#6cc47a', '#a070f0'];

/** Colours cycle over riding days; rest days take no colour. */
export function dayColor(ridingDay: number, theme: 'light' | 'dark' = 'light'): string {
    return (theme === 'dark' ? dark : light)[(ridingDay - 1) % 4];
}
