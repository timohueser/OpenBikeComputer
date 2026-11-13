/** The numbers of a recorded ride from its samples: heart rate, power and cadence with their zones, the climbs, and the
 * table of the data. */
import type { FitSample } from '../core/fit';
import { metres, movingSteps } from '../library/summary';
import { cumulative, firstIndex, type Coordinate } from '../planner/geo';
import { lineClimbs, type Climb } from '../planner/route-facts';
import { heartRateZones, powerZones, type Zone } from '../settings/zones';

/** The mean and the maximum of one sensor while the ride moves, and with a limit the seconds in each zone. */
export interface Channel { mean: number; max: number; zones?: { zone: Zone; seconds: number }[] }

/** A sensor with no value while the ride moves has no channel. */
export interface Channels { hr?: Channel; power?: Channel; cadence?: Channel }

/** A climb with its moving seconds, and its mean power when the ride has power. */
export interface RideClimb extends Climb { seconds: number; powerW?: number }

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
        if (zones && seconds) seconds[zones.filter(zone => v >= zone.from).length - 1] += w;
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

/** The climbs of the ride by the one climb rule (`lineClimbs`), over the line of all samples. */
export function rideClimbs(samples: FitSample[]): RideClimb[] {
    const coordinates = samples.map((s): Coordinate => [s.lon / 1e6, s.lat / 1e6]);
    const km = cumulative(coordinates), steps = movingSteps(samples);
    // `routeSegments` rounds the ends of a climb to 1 m.
    const at = (distance: number) => firstIndex(km.length, i => km[i] >= distance - 5e-4);
    return lineClimbs({ coordinates, elevation: samples.map(s => s.elevationM ?? null), edges: {} }).map(climb => {
        let seconds = 0, energy = 0, powered = 0;
        for (let i = at(climb.from) + 1, last = at(climb.to); i <= last; i++) {
            seconds += steps[i];
            const power = samples[i].powerW;
            if (power !== undefined && steps[i]) { energy += power * steps[i]; powered += steps[i]; }
        }
        return { ...climb, seconds, ...powered && { powerW: energy / powered } };
    });
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
