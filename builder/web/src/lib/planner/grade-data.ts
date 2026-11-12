/** Display grades use a 100 m window within each continuous stretch of known terrain. */
export function profileGrades(samples: readonly { progress: number; height: number | null }[], totalKm: number): (number | null)[] {
    const grades = Array<number | null>(Math.max(0, samples.length - 1)).fill(null);
    if (totalKm <= 0) return grades;
    const distance = samples.map(sample => sample.progress * totalKm * 1000);
    let start = 0;
    while (start < samples.length) {
        if (samples[start].height === null) { start++; continue; }
        let end = start;
        while (end + 1 < samples.length && samples[end + 1].height !== null) end++;
        let left = start, right = start;
        function heightAt(index: number, at: number) {
            const next = Math.min(index + 1, end);
            const length = distance[next] - distance[index];
            return samples[index].height! + (length ? (at - distance[index]) / length : 0) * (samples[next].height! - samples[index].height!);
        }
        for (let i = start; i < end; i++) {
            if (distance[i + 1] <= distance[i]) continue;
            const middle = (distance[i] + distance[i + 1]) / 2;
            const from = Math.max(distance[start], middle - 50), to = Math.min(distance[end], middle + 50);
            while (left < end - 1 && distance[left + 1] <= from) left++;
            while (right < end - 1 && distance[right + 1] <= to) right++;
            // Short fragments cannot support a useful terrain grade estimate.
            if (to - from >= 20) grades[i] = (heightAt(right, to) - heightAt(left, from)) / (to - from) * 100;
        }
        start = end + 1;
    }
    return grades;
}

// The steepness bands of the device; the colours are those of the phone app, readable on a light page.
export const gradeBands = [
    { color: '#4e9a55', dark: '#6cc47a' },
    { color: '#d2ae2a', dark: '#e8c44a' },
    { color: '#f4a81d', dark: '#f2a93a' },
    { color: '#e0782a', dark: '#f29248' },
    { color: '#c43a2c', dark: '#f06a55' },
    { color: '#77746a', dark: '#b8b5ac' },
];

const thresholds = [3, 6, 9, 12];

/** Rounds like `formatGrade`, then cuts toward zero to whole percent like the device; a descent is band 0. */
export function gradeBand(grade: number | null): number {
    if (grade === null || !Number.isFinite(grade)) return 5;
    const percent = Math.trunc(Math.round(grade * 10) / 10);
    return thresholds.filter(threshold => percent >= threshold).length;
}

export function formatGrade(grade: number | null): string {
    if (grade === null) return 'Grade unknown';
    const rounded = Math.round(grade * 10) / 10;
    return `${rounded > 0 ? '+' : rounded < 0 ? '−' : ''}${Math.abs(rounded).toFixed(1)}% grade`;
}
