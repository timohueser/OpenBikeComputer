#ifndef OBC_ROUTE_H
#define OBC_ROUTE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
    int32_t lon, lat;
    int16_t elevation;
    uint8_t surface, elevation_incomplete;
} ObcRoutePoint;

typedef struct {
    double raw_distance_m;
    int32_t lon, lat;
    int16_t lateral_offset_m;
    uint8_t category, name_len;
    uint8_t name[24];
    uint8_t has_provenance, store[16];
    uint64_t object, revision;
    uint16_t ordinal;
} ObcRouteWaypoint;

typedef struct ObcEncodedRoute ObcEncodedRoute;

/* Inputs live for this synchronous call. NULL is failure: error is 1 invalid,
 * 2 capacity, 3 empty line, or 4 sink failure. A successful handle owns its bytes.
 * Copy those bytes before freeing the handle; no pointer survives free. */
ObcEncodedRoute *obc_format_route_encode(
    const ObcRoutePoint *points, size_t point_count,
    const ObcRouteWaypoint *waypoints, size_t waypoint_count,
    const uint8_t *name, size_t name_len, uint8_t bike, int32_t *error);
const uint8_t *obc_format_route_data(const ObcEncodedRoute *route);
size_t obc_format_route_len(const ObcEncodedRoute *route);
void obc_format_route_free(ObcEncodedRoute *route);

#ifdef __cplusplus
}
#endif

#endif
