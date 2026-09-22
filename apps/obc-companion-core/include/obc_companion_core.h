/* The companion core's C surface: verified map cells assembled into one in-memory map, and routes
 * planned over it by the device's router. `apps/obc-companion-core/src/ffi.rs` implements exactly
 * these declarations.
 *
 * A map is immutable, and every route builds its own working state, so any thread may call. A
 * NULL map or route is a no-op, 0, or a failure. obc_core_last_error() is thread-local: read it on
 * the thread that made the failing call. A panic in the core is caught and returned as a failure. */
#ifndef OBC_COMPANION_CORE_H
#define OBC_COMPANION_CORE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct ObcCoreMap ObcCoreMap;
typedef struct ObcCoreRoute ObcCoreRoute;

#define OBC_CORE_NO_ELEVATION INT16_MIN

typedef struct {
    int32_t lon_udeg;
    int32_t lat_udeg;
    int16_t ele_m;             /* OBC_CORE_NO_ELEVATION where the map has no terrain */
    uint8_t surface;           /* surface class of the incoming segment; 0 unknown */
    bool elevation_incomplete;
} ObcCorePoint;

enum {
    OBC_CORE_ROUTED = 0,
    OBC_CORE_NO_ROAD = 1,      /* no road within 100 m of an endpoint */
    OBC_CORE_NO_PATH = 2,
    OBC_CORE_EXHAUSTED = 3,    /* the device's node table filled first: too far */
    OBC_CORE_FAILED = 4,       /* the core panicked; obc_core_last_error() says why */
};

/* catalog_json is the catalog root (OBCC §3): its schema, first skin and terrain lattice.
   The job is JSON: {"cells": [{"id", "band", "partial", "path"}], "known_empty": [{"id", "band"}],
   "terrain": [{"id", "sha256", "path"}]}. Every file is already verified against the catalog.
   NULL on failure. */
ObcCoreMap *obc_core_assemble(const char *catalog_json, const char *job_json);
void obc_core_map_free(ObcCoreMap *map);

/* Coordinates are microdegrees; bike is the bike type (OBCR §1.2, 0..=3). On OBC_CORE_ROUTED,
   *out is a route the caller frees. */
int32_t obc_core_route(const ObcCoreMap *map, int32_t from_lon, int32_t from_lat, int32_t to_lon,
                       int32_t to_lat, uint8_t bike, ObcCoreRoute **out);
const ObcCorePoint *obc_core_route_points(const ObcCoreRoute *route, size_t *count); /* valid while the route lives */
uint32_t obc_core_route_distance_m(const ObcCoreRoute *route);
uint32_t obc_core_route_ascent_m(const ObcCoreRoute *route);
void obc_core_route_free(ObcCoreRoute *route);

const char *obc_core_last_error(void); /* empty until a call fails; valid until the next failure */

#ifdef __cplusplus
}
#endif

#endif /* OBC_COMPANION_CORE_H */
