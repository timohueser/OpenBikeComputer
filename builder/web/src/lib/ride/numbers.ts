/** The numbers of a recorded ride from its samples: heart rate, power and cadence with their zones, the climbs, and the
 * table of the data. */
import type { FitSample } from '../core/fit';
import { metres, movingSteps, rideTotals } from '../library/summary';
import { cumulative, firstIndex, type Coordinate } from '../planner/geo';
import { routeSegments } from '../planner/search/segments';
import { heartRateZones, powerZones, zoneOf, type Zone } from '../settings/zones';

/** The mean and the maximum of one sensor while the ride moves, and with a limit the seconds in each zone. */
export interface Channel { mean: number; max: number; zones?: { zone: Zone; seconds: number }[] }

/** A sensor with no value while the ride moves has no channel. */
export interface Channels { hr?: Channel; power?: Channel; cadence?: Channel }

/** A climb from sample `first` to sample `last`. `from` and `to` are kilometres from the start, with no hop across a
 * pause; `top` is the place of the last sample. `seconds` is its moving time, `powerW` its mean power when the ride has
 * power. */
export interface RideClimb { first: number; last: number; from: number; to: number; ascent: number; top: Coordinate; seconds: number; powerW?: number }

/** Each sample weighs the seconds of the moving step that ends at it (`movingSteps`). */
function channel(samples: FitSample[], steps: number[], value: (sample: FitSample) => number | undefined, zones?: Zone[]): Channel | undefined {
    let sum = 0, weight = 0, max = -Infinity;
    const seconds = zones?.map(() => 0);
    samples.forEach((sample, i) => {
        const v = value(sample), w = steps[i];
        if (v === undefined || !w) return;
        sum += v * w;
        weight += w;
        max = Math.max(max, v);
        if (zones && seconds) seconds[zoneOf(zones, v)] += w;
    });
    if (!weight) return undefined;
    return { mean: sum / weight, max, ...zones && seconds && { zones: zones.map((zone, i) => ({ zone, seconds: seconds[i] })) } };
}

/** Heart rate, power and cadence over the moving steps of the ride. A limit that is null gives no zones. A cadence of 0
 * (coasting) does not count. */
export function channels(samples: FitSample[], limits: { maxHr: number | null; ftp: number | null }): Channels {
    const steps = movingSteps(samples);
    return {
        hr: channel(samples, steps, s => s.hrBpm, limits.maxHr ? heartRateZones(limits.maxHr) : undefined),
        power: channel(samples, steps, s => s.powerW, limits.ftp ? powerZones(limits.ftp) : undefined),
        cadence: channel(samples, steps, s => s.cadenceRpm || undefined),
    };
}

/** The speed over the moving steps: their metres over their seconds, as the device gives it. Undefined with no moving
 * step. */
export function movingSpeed(samples: FitSample[]): number | undefined {
    let distance = 0, seconds = 0;
    movingSteps(samples).forEach((step, i) => {
        if (step) { distance += metres(samples[i - 1], samples[i]); seconds += step; }
    });
    return seconds ? distance / seconds : undefined;
}

/** The numbers of a stretch of a ride. */
export interface PartNumbers { lengthM: number; climbM: number; movingS: number; speed?: number; channels: Channels }

/** The numbers of the samples `first` to `last` as a ride of its own. Its first sample starts a segment, so no step
 * leads into the part. */
export function partNumbers(samples: FitSample[], [first, last]: [number, number], limits: { maxHr: number | null; ftp: number | null }): PartNumbers {
    const part = samples.slice(first, last + 1);
    part[0] = { ...part[0], segmentStart: true };
    return { ...rideTotals(part), speed: movingSpeed(part), channels: channels(part, limits) };
}

/** The climbs of the ride by the one climb rule (`routeSegments`), found in each segment by itself, so that a pause
 * neither adds the hop to a climb nor joins two climbs. */
export function rideClimbs(samples: FitSample[]): RideClimb[] {
    const steps = movingSteps(samples), along = [0];
    for (let i = 1; i < samples.length; i++) along.push(along[i - 1] + (samples[i].segmentStart ? 0 : metres(samples[i - 1], samples[i])));
    const climbs: RideClimb[] = [];
    for (let start = 0; start < samples.length;) {
        let end = start + 1;
        while (end < samples.length && !samples[end].segmentStart) end++;
        const part = samples.slice(start, end), coordinates = part.map((s): Coordinate => [s.lon / 1e6, s.lat / 1e6]), km = cumulative(coordinates);
        // `routeSegments` rounds the ends of a climb to 1 m.
        const at = (distance: number) => start + firstIndex(km.length, i => km[i] >= distance - 5e-4);
        const found = part.length < 2 ? [] : routeSegments({ coordinates, elevation: part.map(s => s.elevationM ?? null), edges: {} });
        for (const { kind, from, to, ascent } of found) {
            if (kind !== 'climb') continue;
            const first = at(from), last = at(to);
            let seconds = 0, energy = 0, powered = 0;
            for (let i = first + 1; i <= last; i++) {
                seconds += steps[i];
                const power = samples[i].powerW;
                if (power !== undefined && steps[i]) { energy += power * steps[i]; powered += steps[i]; }
            }
            climbs.push({ first, last, from: along[first] / 1000, to: along[last] / 1000, ascent: ascent!, top: coordinates[last - start], seconds,
                ...powered && { powerW: energy / powered } });
        }
        start = end;
    }
    return climbs;
}

const CSV_HEADER = 'time,latitude,longitude,elevation_m,distance_m,speed_m_s,heart_rate_bpm,cadence_rpm,power_w,temperature_c';

/** One line for each sample, in SI units and UTC. `distance_m` skips the step to a segment start. A missing value is an
 * empty field. */
export function rideCsv(samples: FitSample[]): string {
    const cell = (value: number | undefined, digits: number) => value === undefined ? '' : String(Math.round(value * 10 ** digits) / 10 ** digits);
    let distance = 0;
    const rows = samples.map((s, i) => {
        if (i && !s.segmentStart) distance += metres(samples[i - 1], s);
        return [new Date(s.timeS * 1000).toISOString(), (s.lat / 1e6).toFixed(6), (s.lon / 1e6).toFixed(6), cell(s.elevationM, 1), cell(distance, 1),
            cell(s.speedMps, 3), cell(s.hrBpm, 0), cell(s.cadenceRpm, 0), cell(s.powerW, 0), cell(s.temperatureC, 1)].join(',');
    });
    return [CSV_HEADER, ...rows, ''].join('\n');
}
