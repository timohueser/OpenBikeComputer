const light = ['#cc2a93', '#2f6fb5', '#3b8a3f', '#5b2fb0'];
const dark = ['#e45cb5', '#6fa8e8', '#6cc47a', '#a070f0'];

export function dayColor(day: number, theme: 'light' | 'dark' = 'light'): string {
    return (theme === 'dark' ? dark : light)[(day - 1) % 4];
}
