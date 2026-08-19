//! Allocation-free selection of an effective object from head/retained catalog revisions.

/// One catalog revision after the storage adapter attempted domain validation.
///
/// `Ok(Some(value))` is valid, `Ok(None)` is definitively malformed, and `Err(error)` is a
/// retryable read/open failure. Revisions must arrive in the flat catalog's `(ObjectId, Revision)`
/// order, where a retained predecessor immediately precedes its head.
pub struct CatalogRevision<T, E> {
    pub object_id: u64,
    pub retained: bool,
    pub validation: Result<Option<T>, E>,
}

/// Select one effective value without allocating a catalog copy.
///
/// A valid head always wins for its object. A malformed head falls back to its valid retained
/// predecessor; a read failure on the head, or on a fallback that is actually needed, is returned
/// so the caller can preserve its currently mounted value and retry. Across effective per-object
/// candidates, `incoming_wins` supplies the domain's serial/timestamp/tie-break rule.
pub fn select_catalog<T: Copy, E, I, F>(revisions: I, mut incoming_wins: F) -> Result<Option<T>, E>
where
    I: IntoIterator<Item = CatalogRevision<T, E>>,
    F: FnMut(T, T) -> bool,
{
    let mut active = None;
    let mut retained = None;
    for revision in revisions {
        if revision.retained {
            retained = Some((revision.object_id, revision.validation));
            continue;
        }
        let incoming = match revision.validation? {
            Some(head) => Some(head),
            None => match retained.take() {
                Some((object_id, fallback)) if object_id == revision.object_id => fallback?,
                _ => None,
            },
        };
        retained = None;
        if let Some(incoming) = incoming {
            if active.is_none_or(|current| incoming_wins(incoming, current)) {
                active = Some(incoming);
            }
        }
    }
    Ok(active)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ReadError {
        Media,
    }

    fn revision(
        object_id: u64,
        retained: bool,
        validation: Result<Option<u8>, ReadError>,
    ) -> CatalogRevision<u8, ReadError> {
        CatalogRevision { object_id, retained, validation }
    }

    #[test]
    fn no_weather_is_none() {
        let revisions: [CatalogRevision<u8, ReadError>; 0] = [];
        assert_eq!(select_catalog(revisions, |_, _| false), Ok(None));
    }

    #[test]
    fn a_valid_head_wins_even_when_the_retained_value_would_rank_newer() {
        let revisions = [revision(4, true, Ok(Some(9))), revision(4, false, Ok(Some(3)))];
        assert_eq!(select_catalog(revisions, |incoming, current| incoming > current), Ok(Some(3)));
    }

    #[test]
    fn a_malformed_head_falls_back_to_its_valid_retained_predecessor() {
        let revisions = [revision(4, true, Ok(Some(9))), revision(4, false, Ok(None))];
        assert_eq!(select_catalog(revisions, |incoming, current| incoming > current), Ok(Some(9)));
    }

    #[test]
    fn a_retained_read_failure_is_ignored_when_the_head_is_valid() {
        let revisions = [revision(4, true, Err(ReadError::Media)), revision(4, false, Ok(Some(3)))];
        assert_eq!(select_catalog(revisions, |incoming, current| incoming > current), Ok(Some(3)));
    }

    #[test]
    fn a_head_or_required_fallback_read_failure_is_retryable() {
        assert_eq!(select_catalog([revision(4, false, Err(ReadError::Media))], |_, _| false), Err(ReadError::Media));
        let revisions = [revision(4, true, Err(ReadError::Media)), revision(4, false, Ok(None))];
        assert_eq!(select_catalog(revisions, |_, _| false), Err(ReadError::Media));
    }

    #[test]
    fn fallback_never_crosses_an_object_identity_and_effective_heads_use_the_domain_order() {
        let revisions = [
            revision(4, true, Ok(Some(9))),
            revision(5, false, Ok(None)),
            revision(6, false, Ok(Some(2))),
            revision(7, false, Ok(Some(8))),
        ];
        assert_eq!(select_catalog(revisions, |incoming, current| incoming > current), Ok(Some(8)));
    }
}
