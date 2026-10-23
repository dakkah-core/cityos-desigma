//! Current editor theme and UI hooks passed into the catalog extension.
use crate::theme;
use desigma_cityos::catalog::{CatalogStyle, CatalogUiServices};

pub(super) fn services() -> CatalogUiServices {
    CatalogUiServices {
        style: catalog_style,
        record: crate::widgets::record,
        specimen_renderer: |document| Box::new(super::canvas::SpecimenCanvas::new(document)),
        pick_repository: || rfd::FileDialog::new().pick_folder(),
        pick_reader: || rfd::FileDialog::new().add_filter("CityOS reader", &["mjs"]).pick_file(),
    }
}

fn catalog_style() -> CatalogStyle {
    CatalogStyle {
        bg: theme::BG,
        bg_hover: theme::BG_HOVER,
        bg_secondary: theme::BG_SECONDARY,
        bg_selected_secondary: theme::BG_SELECTED_SECONDARY,
        bg_tertiary: theme::BG_TERTIARY,
        border_selected: theme::BORDER_SELECTED,
        catalog_body: theme::CATALOG_BODY,
        catalog_gap: theme::CATALOG_GAP,
        catalog_list_width: theme::CATALOG_LIST_WIDTH,
        catalog_narrow: theme::CATALOG_NARROW,
        catalog_nav_row: theme::CATALOG_NAV_ROW,
        catalog_nav_width: theme::CATALOG_NAV_WIDTH,
        catalog_pad: theme::CATALOG_PAD,
        catalog_radius: theme::CATALOG_RADIUS,
        catalog_row: theme::CATALOG_ROW,
        catalog_small: theme::CATALOG_SMALL,
        catalog_stack: theme::CATALOG_STACK,
        catalog_title: theme::CATALOG_TITLE,
        danger: theme::DANGER,
        text: theme::TEXT,
        text_secondary: theme::TEXT_SECONDARY,
        text_tertiary: theme::TEXT_TERTIARY,
    }
}
