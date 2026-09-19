//! A card image: the flat store mounted over the file, exactly as the device mounts a card.

use obc_formats::io::ByteSource;
use obc_storage::flat::{FlatStore, Store};

use crate::report::{bytes, Report};
use crate::source::FileSource;

pub fn report(source: &FileSource) -> Result<Report, String> {
    let store = FlatStore::mount(source);
    let mut out = Report::new();
    out.put("bytes", bytes(source.len()));

    let mut header = Report::new();
    header
        .put("store_id", hex(&store.store_id().0))
        .put("mode", format!("{:?}", store.mode()))
        .put("extent_size", bytes(store.extent_size()))
        .put("free_extents", store.free_extents());
    out.group("store", header);

    if !store.mode().readable() {
        // Every catalog fact below comes from a served copy; there is none.
        return Ok(out);
    }

    let mut catalog = Report::new();
    catalog
        .put("serving_copy", if store.serving_copy() == 0 { "A" } else { "B" })
        .put("sequence", store.sequence())
        .put("high_water", store.high_water())
        .put("entries", store.entry_count())
        .put("next_object_id", store.next_object_id().0);
    out.group("catalog", catalog);

    let objects: Vec<Report> = store
        .entries()
        .map(|entry| {
            let mut row = Report::new();
            row.put("id", entry.id.0)
                .put("revision", entry.revision.0)
                .put("kind", format!("{:?}", entry.kind))
                .put("name", entry.name.as_str().unwrap_or("(not text)"))
                .put("payload", bytes(entry.payload_len))
                .put("flags", format!("0x{:04x}", entry.flags.bits()))
                .put("added_at_utc", entry.added_at_utc);
            row
        })
        .collect();
    // The listing is read block by block; a failure part way through is a fact about the card.
    out.put("listing_complete", store.entries_ok());
    out.list("objects", objects);

    let mut recovery = Report::new();
    match store.recovered_ride() {
        Some(ride) => {
            recovery
                .put("present", true)
                .put("id", ride.id.0)
                .put("revision", ride.revision.0)
                .put("checkpoint", ride.checkpoint_sequence)
                .put("payload", bytes(ride.payload_len()));
        }
        None => {
            recovery.put("present", false);
        }
    }
    out.group("recovered_ride", recovery);
    Ok(out)
}

fn hex(id: &[u8; 16]) -> String {
    id.iter().map(|byte| format!("{byte:02x}")).collect()
}
