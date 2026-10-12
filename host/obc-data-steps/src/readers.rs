//! The programs that read the layers, and what a new format of a layer means for each.

use obc_data::engine::Reader;

/// The planner services wait at Router ready, as `specs/obc-data.md` says.
macro_rules! service {
    ($name:literal) => {
        Reader {
            name: $name,
            on_new_format: concat!(
                "The run uploads, then waits at Router ready, at most 24 hours, until the follower runs a ",
                $name,
                " that reads the new format. The target does not change before."
            ),
        }
    };
}

pub static DEVICE: Reader = Reader {
    name: "device",
    on_new_format: "Maps already on a device keep working. The map builder gives new maps only to firmware that reads the new format.",
};
pub static MAP_BUILDER: Reader = Reader {
    name: "map builder",
    on_new_format: "An old map builder reads no new data: it needs a build that reads the new format.",
};
pub static DESKTOP_APP: Reader = Reader {
    name: "desktop app",
    on_new_format: "An old desktop app reads no new data: it needs a build that reads the new format.",
};
pub static WEB_PLANNER: Reader = Reader {
    name: "web planner",
    on_new_format: "The web planner reads the new data after a deploy of code that reads the new format.",
};
pub static IOS: Reader = Reader {
    name: "iOS",
    on_new_format: "Data already on a phone keeps working. New downloads need an app that reads the new format.",
};
pub static IOS_OFFLINE_ROUTER: Reader = Reader {
    name: "iOS offline router",
    on_new_format:
        "Offline areas already on a phone keep working. New offline downloads need an app that reads the new format.",
};
pub static ROUTER_SERVICE: Reader = service!("router service");
pub static SEARCH_SERVICE: Reader = service!("search service");
pub static DOWNLOADS_SERVICE: Reader = service!("downloads service");
pub static PLANNER_SUN: Reader =
    Reader { name: "planner/sun", on_new_format: "planner/sun bakes again from the new data in the same update." };
