//! Compact, read-only accounting from a catalog generation already loaded by Browser.
//! This is deliberately separate from exchange resources and their capabilities.
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct CoverageLevel {
    pub id: String,
    pub name: String,
    pub records: Option<u64>,
    pub data_specimens: Option<u64>,
    pub conflicting: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CoverageObservation {
    pub repository_id: String,
    pub generation_id: String,
    pub canonical_commit: String,
    pub mode: String,
    pub reader_identity: String,
    pub accounting_complete: Option<bool>,
    pub stale: bool,
    pub source_occurrences: Option<u64>,
    pub library_records: Option<u64>,
    pub theme_records: Option<u64>,
    pub token_records: Option<u64>,
    pub levels: Vec<CoverageLevel>,
}

fn digest(value: &Value, lengths: &[usize]) -> Option<String> {
    let value = value.as_str()?;
    (lengths.contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())).then(|| value.to_owned())
}

impl CoverageObservation {
    pub(super) fn from_summary(summary: &Value, stale: bool) -> Option<Self> {
        let snapshot = &summary["snapshot"];
        let mode = snapshot["mode"].as_str().filter(|mode| matches!(*mode, "committed" | "local"))?;
        let rows = summary["navigation"]["levels"].as_array();
        let collections = summary["navigation"]["collections"].as_array();
        let collection = |id: &str| {
            let members: Vec<_> = collections.into_iter().flatten().filter(|row| row["id"] == id).collect();
            (members.len() == 1).then(|| members[0]["count"].as_u64()).flatten()
        };
        Some(Self {
            repository_id: digest(&snapshot["repositoryId"], &[64])?,
            generation_id: digest(&summary["generationId"], &[64])?,
            canonical_commit: digest(&snapshot["canonicalCommit"], &[40, 64])?,
            reader_identity: digest(&snapshot["readerImplementation"], &[64])?,
            mode: mode.into(),
            accounting_complete: summary["accountingComplete"].as_bool(),
            stale: stale || summary["stale"] == true,
            source_occurrences: summary["entryCount"].as_u64(),
            library_records: collection("design"),
            theme_records: collection("themes"),
            token_records: collection("tokens"),
            levels: (0..10)
                .map(|index| {
                    let id = format!("L{index}");
                    let members: Vec<_> = rows.into_iter().flatten().filter(|row| row["id"] == id).collect();
                    let row = (members.len() == 1).then(|| members[0]);
                    CoverageLevel {
                        id,
                        name: row.and_then(|row| row["name"].as_str()).unwrap_or("Not reported").chars().take(160).collect(),
                        records: row.and_then(|row| row["count"].as_u64()),
                        data_specimens: row.and_then(|row| row["temporaryViewCount"].as_u64()),
                        conflicting: members.len() > 1,
                    }
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub(super) fn summary() -> Value {
        json!({"generationId":"a".repeat(64),"entryCount":500,"accountingComplete":true,
            "snapshot":{"repositoryId":"b".repeat(64),"canonicalCommit":"c".repeat(40),"readerImplementation":"d".repeat(64),"mode":"committed"},
            "navigation":{"collections":[{"id":"design","count":40},{"id":"themes","count":3},{"id":"tokens","count":31}],
                "levels":[{"id":"L0","name":"Foundation","count":34,"temporaryViewCount":34},{"id":"L1","name":"Primitive","count":0,"temporaryViewCount":0}]}})
    }

    #[test]
    fn all_levels_keep_missing_counts_distinct_from_observed_zero_and_census_totals() {
        let report = CoverageObservation::from_summary(&summary(), false).unwrap();
        assert_eq!(report.levels.len(), 10);
        assert_eq!(report.levels[0].records, Some(34));
        assert_eq!(report.levels[1].records, Some(0));
        assert_eq!(report.levels[2].records, None);
        assert_eq!(report.source_occurrences, Some(500));
        assert_eq!(report.library_records, Some(40));
        assert_eq!(report.theme_records, Some(3));
        assert_eq!(report.token_records, Some(31));
        assert_eq!(report.accounting_complete, Some(true));
    }

    #[test]
    fn conflicts_and_malformed_or_absent_counts_never_choose_a_winner_or_invent_zero() {
        let mut value = summary();
        let duplicate = value["navigation"]["levels"][0].clone();
        value["navigation"]["levels"].as_array_mut().unwrap().push(duplicate);
        value["navigation"]["collections"].as_array_mut().unwrap().push(json!({"id":"themes","count":99}));
        value["entryCount"] = json!(-1);
        value.as_object_mut().unwrap().remove("accountingComplete");
        let report = CoverageObservation::from_summary(&value, false).unwrap();
        assert!(report.levels[0].conflicting);
        assert_eq!(report.levels[0].records, None);
        assert_eq!(report.theme_records, None);
        assert_eq!(report.source_occurrences, None);
        assert_eq!(report.accounting_complete, None);
    }

    #[test]
    fn source_identity_and_mode_are_required_and_failed_refresh_remains_visible() {
        for path in ["generationId", "snapshot.repositoryId", "snapshot.canonicalCommit", "snapshot.readerImplementation", "snapshot.mode"] {
            let mut value = summary();
            if let Some((parent, key)) = path.split_once('.') {
                value[parent][key] = json!("invalid");
            } else {
                value[path] = json!("invalid");
            }
            assert!(CoverageObservation::from_summary(&value, false).is_none(), "{path}");
        }
        let mut value = summary();
        value["snapshot"]["mode"] = json!("local");
        value["accountingComplete"] = json!(false);
        let report = CoverageObservation::from_summary(&value, true).unwrap();
        assert_eq!(report.mode, "local");
        assert_eq!(report.accounting_complete, Some(false));
        assert!(report.stale);
        value["stale"] = json!(true);
        assert!(CoverageObservation::from_summary(&value, false).unwrap().stale);
    }

    #[test]
    fn browser_reuses_only_its_bound_generation_without_queueing_work() {
        use super::super::{test_support, CatalogBinding};
        let (mut browser, _, jobs) = test_support::captured_browser(test_support::services());
        let value = summary();
        browser.summary = Some(value.clone());
        browser.catalog_binding = Some(CatalogBinding { config: browser.reader_config(), mode: "committed".into(), generation: "a".repeat(64) });
        let first = browser.coverage_observation().unwrap();
        assert_eq!(first.theme_records, Some(3));
        assert!(jobs.try_recv().is_err(), "Coverage must not dispatch a scan or query");
        browser.error = Some("Remote refresh unavailable".into());
        assert!(browser.coverage_observation().unwrap().stale);
        browser.summary.as_mut().unwrap()["generationId"] = serde_json::json!("e".repeat(64));
        assert!(browser.coverage_observation().is_none());
        browser.summary = Some(value);
        browser.mode = "local".into();
        assert!(browser.coverage_observation().is_none());
        browser.mode = "committed".into();
        browser.repository.push_str("-other");
        assert!(browser.coverage_observation().is_none(), "A context change cannot retain another repository's observation");
        assert!(jobs.try_recv().is_err());
    }
}
