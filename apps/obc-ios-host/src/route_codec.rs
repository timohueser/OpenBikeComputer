//! Synchronous route encoding for the Swift companion. The caller copies the result before
//! freeing its handle. Format and geometry rules belong to `obc-route`.

use obc_formats::io::{ByteSink, Error};
use obc_route::{BikeType, RoutePoint, Waypoint};
use std::ptr;

#[repr(C)]
pub struct ObcRoutePoint {
    pub lon: i32,
    pub lat: i32,
    pub elevation: i16,
    pub surface: u8,
    pub elevation_incomplete: u8,
}

#[repr(C)]
pub struct ObcRouteWaypoint {
    pub raw_distance_m: f64,
    pub lon: i32,
    pub lat: i32,
    pub lateral_offset_m: i16,
    pub category: u8,
    pub name_len: u8,
    pub name: [u8; 24],
    pub has_provenance: u8,
    pub store: [u8; 16],
    pub object: u64,
    pub revision: u64,
    pub ordinal: u16,
}

pub struct ObcEncodedRoute(Vec<u8>);

impl ByteSink for ObcEncodedRoute {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.0.extend_from_slice(bytes);
        Ok(())
    }

    fn patch_at(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Error> {
        let start = offset as usize;
        let end = start.checked_add(bytes.len()).ok_or(Error::BadOffset)?;
        self.0.get_mut(start..end).ok_or(Error::BadOffset)?.copy_from_slice(bytes);
        Ok(())
    }
}

/// # Safety
/// A nonempty input is aligned, readable for its stated length and alive for the call.
unsafe fn input<'a, T>(data: *const T, len: usize) -> Result<&'a [T], Error> {
    if len == 0 {
        return Ok(&[]);
    }
    if data.is_null() || len > isize::MAX as usize / size_of::<T>() {
        return Err(Error::BadOffset);
    }
    Ok(unsafe { std::slice::from_raw_parts(data, len) })
}

/// Encode authored geometry. NULL means invalid, empty or too large. `error` receives 0 on
/// success, 1 for invalid input, 2 for capacity, 3 for an empty line, or 4 for sink failure.
///
/// # Safety
/// Inputs are aligned readable arrays alive for the call. `error` is NULL or writable. The
/// returned handle must be freed once, after all reads through its data pointer end.
#[no_mangle]
pub unsafe extern "C" fn obc_format_route_encode(
    points: *const ObcRoutePoint,
    point_count: usize,
    waypoints: *const ObcRouteWaypoint,
    waypoint_count: usize,
    name: *const u8,
    name_len: usize,
    bike: u8,
    error: *mut i32,
) -> *mut ObcEncodedRoute {
    let encode = || -> Result<ObcEncodedRoute, Error> {
        if waypoint_count > obc_route::MAX_WAYPOINTS {
            return Err(Error::TooLarge);
        }
        let name = std::str::from_utf8(unsafe { input(name, name_len)? }).map_err(|_| Error::BadOffset)?;
        let bike = BikeType::from_u8(bike).ok_or(Error::BadOffset)?;
        let points = unsafe { input(points, point_count)? }
            .iter()
            .map(|point| {
                if point.elevation_incomplete > 1 {
                    return Err(Error::BadOffset);
                }
                Ok(RoutePoint {
                    lon: point.lon,
                    lat: point.lat,
                    ele: point.elevation,
                    surface: point.surface,
                    elevation_incomplete: point.elevation_incomplete != 0,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let waypoints = unsafe { input(waypoints, waypoint_count)? }
            .iter()
            .map(|point| {
                let bytes = point.name.get(..usize::from(point.name_len)).ok_or(Error::BadOffset)?;
                let name = std::str::from_utf8(bytes).map_err(|_| Error::BadOffset)?;
                let mut waypoint = Waypoint {
                    dist_along_m: 0,
                    lon: point.lon,
                    lat: point.lat,
                    ele: obc_formats::obcr::WAYPOINT_ELE_NONE,
                    category_id: point.category,
                    lateral_offset_m: point.lateral_offset_m,
                    name: Default::default(),
                    provenance: match point.has_provenance {
                        0 => None,
                        1 if point.object != 0 && point.revision != 0 => Some(obc_formats::obcr::WaypointProvenance {
                            source: obc_formats::obcr::RouteSourceKey {
                                store: point.store,
                                object: point.object,
                                revision: point.revision,
                            },
                            ordinal: point.ordinal,
                        }),
                        _ => return Err(Error::BadOffset),
                    },
                };
                waypoint.name.push_str(name).map_err(|_| Error::TooLarge)?;
                Ok((waypoint, point.raw_distance_m))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let mut out = ObcEncodedRoute(Vec::new());
        obc_route::convert::points_to_obcr(&points, &waypoints, name, bike, &mut out)?;
        Ok(out)
    };
    let (result, code) = match encode() {
        Ok(bytes) => (Box::into_raw(Box::new(bytes)), 0),
        Err(error) => (
            ptr::null_mut(),
            match error {
                Error::TooLarge => 2,
                Error::Empty => 3,
                Error::Io => 4,
                _ => 1,
            },
        ),
    };
    if let Some(error) = unsafe { error.as_mut() } {
        *error = code;
    }
    result
}

/// # Safety
/// `route` is NULL or a live handle; the returned bytes expire when the handle is freed.
#[no_mangle]
pub unsafe extern "C" fn obc_format_route_data(route: *const ObcEncodedRoute) -> *const u8 {
    unsafe { route.as_ref() }.map_or(ptr::null(), |route| route.0.as_ptr())
}

/// # Safety
/// `route` is NULL or a live handle.
#[no_mangle]
pub unsafe extern "C" fn obc_format_route_len(route: *const ObcEncodedRoute) -> usize {
    unsafe { route.as_ref() }.map_or(0, |route| route.0.len())
}

/// # Safety
/// `route` is NULL or a live handle not previously freed, with no outstanding pointer reads.
#[no_mangle]
pub unsafe extern "C" fn obc_format_route_free(route: *mut ObcEncodedRoute) {
    if !route.is_null() {
        drop(unsafe { Box::from_raw(route) });
    }
}
