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

export const gradeBands = [
    { label: '20%+', color: '#7030a0', dark: '#d79bef' },
    { label: '15–20%', color: '#5757b6', dark: '#b4a1ed' },
    { label: '10–15%', color: '#356cc1', dark: '#8eacf1' },
    { label: '6–10%', color: '#2187bb', dark: '#6fc4ee' },
    { label: '3–6%', color: '#349eac', dark: '#68d0d9' },
    { label: 'Flat · <3%', color: '#38834b', dark: '#86ca8f' },
    { label: '3–6%', color: '#78a528', dark: '#b5d76c' },
    { label: '6–10%', color: '#c4ac16', dark: '#eed558' },
    { label: '10–15%', color: '#ec5036', dark: '#ff987d' },
    { label: '15–20%', color: '#c72232', dark: '#ff7080' },
    { label: '20%+', color: '#95162c', dark: '#ed4967' },
    { label: 'Unknown', color: '#77746a', dark: '#b8b5ac' },
];

const gradeThresholds = [3, 6, 10, 15, 20];

export function gradeBand(grade: number | null): number {
    if (grade === null || !Number.isFinite(grade)) return 11;
    const magnitude = Math.round(Math.abs(grade) * 10) / 10;
    let level = 0;
    while (level < gradeThresholds.length && magnitude >= gradeThresholds[level]) level++;
    return 5 + (grade < 0 ? -level : level);
}

export function formatGrade(grade: number | null): string {
    if (grade === null) return 'Grade unknown';
    const rounded = Math.round(grade * 10) / 10;
    return `${rounded > 0 ? '+' : rounded < 0 ? '−' : ''}${Math.abs(rounded).toFixed(1)}% grade`;
}
