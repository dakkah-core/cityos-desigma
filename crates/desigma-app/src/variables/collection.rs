//! Collection selection is presentation state, independent of variable values.
//! Callers supply only collections still present in the current document.

pub(crate) fn select_collection<'a>(
    current: Option<&'a str>,
    page_changed: bool,
    page_collection: Option<&'a str>,
    first_collection: Option<&'a str>,
) -> Option<&'a str> {
    if page_changed {
        page_collection.or(current).or(first_collection)
    } else {
        current.or(page_collection).or(first_collection)
    }
}

#[cfg(test)]
mod tests {
    use super::select_collection;

    #[test]
    fn first_open_prefers_the_active_theme_page_to_collection_sort_order() {
        assert_eq!(select_collection(None, true, Some("light"), Some("dark")), Some("light"));
    }

    #[test]
    fn changed_theme_page_replaces_the_previous_automatic_collection() {
        assert_eq!(select_collection(Some("light"), true, Some("dark"), Some("light")), Some("dark"));
        assert_eq!(select_collection(Some("dark"), true, Some("light"), Some("dark")), Some("light"));
    }

    #[test]
    fn same_page_preserves_deliberate_collection_browsing() {
        assert_eq!(select_collection(Some("dark"), false, Some("light"), Some("dark")), Some("dark"));
    }

    #[test]
    fn ordinary_pages_without_a_mode_keep_a_valid_collection() {
        assert_eq!(select_collection(Some("chosen"), true, None, Some("first")), Some("chosen"));
    }

    #[test]
    fn removed_selection_falls_back_to_a_live_page_collection_then_a_live_first_collection() {
        assert_eq!(select_collection(None, false, Some("light"), Some("dark")), Some("light"));
        assert_eq!(select_collection(None, false, None, Some("dark")), Some("dark"));
    }

    #[test]
    fn empty_documents_never_retain_a_removed_collection() {
        assert_eq!(select_collection(None, false, None, None), None);
    }
}
