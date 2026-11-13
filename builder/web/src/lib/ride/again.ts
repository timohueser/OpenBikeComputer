/** Opens a ride as a planned route, so the rider can ride it again. */
import type { FitSample } from '../core/fit';
import { initCore } from '../core/bridge';
import { lineId } from '../library/import';
import type { RideRecord } from '../library/records';
import type { Library } from '../library/store';
import { planFromFile } from '../planner/gpx-import';
import { ROUTES } from '../routes';
import { rideSamples } from './samples';

/** Opens the route of the ride's line, with the id that "Add files" gives the line, so a second call opens the same route.
 * It makes the route first if needed, from `samples` or else from the stored file. */
export async function rideAgain(library: Library, ride: RideRecord, samples?: FitSample[]): Promise<void> {
    const id = await lineId(ride.summary.line);
    if (!await library.item(id)) {
        await initCore();
        const file = samples ? undefined : await library.rideFile(ride.key);
        const read = samples ?? (file && await rideSamples(ride, file));
        if (!read?.length) throw new Error('The file of this ride cannot be read.');
        const points = read.map(s => ({ lat: s.lat, lon: s.lon, ...s.elevationM !== undefined && { ele: s.elevationM } }));
        await library.addRoute(await planFromFile({ name: ride.name, points, waypoints: [] }, id, ride.summary.bike));
    }
    location.hash = `${ROUTES.route}/${id}`;
}
