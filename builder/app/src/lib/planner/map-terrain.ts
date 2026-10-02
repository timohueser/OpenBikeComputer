import contour from 'maplibre-contour';
import type * as maplibre from 'maplibre-gl';
import { clientConfig } from './client-config';

/** Keep one bounded DEM cache per map surface; the plugin has no manager disposal API. */
export function terrainSource(url: string) {
    let source: InstanceType<typeof contour.DemSource> | undefined;
    let users = 0;
    return {
        acquire(library: typeof maplibre) {
            if (clientConfig.terrainWorkerUrl) contour.workerUrl = new URL(clientConfig.terrainWorkerUrl, window.location.href).href;
            source ??= new contour.DemSource({ url, maxzoom: 12, worker: true, cacheSize: 64, encoding: 'terrarium' });
            if (users++ === 0) source.setupMaplibre(library);
            let active = true;
            const dem = source;
            return { dem, release() {
                if (!active) return;
                active = false;
                if (--users === 0) {
                    library.removeProtocol(dem.sharedDemProtocolId);
                    library.removeProtocol(dem.contourProtocolId);
                }
            } };
        },
    };
}
