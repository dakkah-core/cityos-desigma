//! Explicit, read-only routing over independent v1 reader sessions.
//!
//! Bindings are caller-selected expectations, not discovery or owner authority.
//! Each cache must already exist and be dedicated to one active selection.
//! Separate federation instances and external processes must coordinate their
//! own cache ownership; this module does not provide a cross-process lease.
use crate::{Client, Filters, Page, ReaderConfig};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub selection_id: String,
    pub repository_id: String,
    pub generation_id: String,
    pub canonical_commit: String,
    pub mode: String,
}

impl Binding {
    /// Bind an already observed generation to an explicit UI selection. Optional
    /// expectations are comparisons only; they do not establish source custody.
    pub fn from_summary(selection_id: &str, mode: &str, summary: &Value, expected_repository: Option<&str>, expected_commit: Option<&str>) -> Result<Self, String> {
        let identity = |value: &str, lengths: &[usize]| lengths.contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit());
        if selection_id.is_empty() || selection_id.len() > 160 || !selection_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
            || !matches!(mode, "committed" | "local") {
            return Err("FEDERATION_SELECTION_INVALID".into());
        }
        let snapshot = &summary["snapshot"];
        let repository_id = snapshot["repositoryId"].as_str().filter(|value| identity(value, &[64])).ok_or("FEDERATION_SUMMARY_INVALID")?;
        let generation_id = summary["generationId"].as_str().filter(|value| identity(value, &[64])).ok_or("FEDERATION_SUMMARY_INVALID")?;
        let canonical_commit = snapshot["canonicalCommit"].as_str().filter(|value| identity(value, &[40, 64])).ok_or("FEDERATION_SUMMARY_INVALID")?;
        if snapshot["mode"].as_str() != Some(mode)
            || expected_repository.is_some_and(|expected| expected != repository_id)
            || expected_commit.is_some_and(|expected| expected != canonical_commit) {
            return Err("FEDERATION_BINDING_MISMATCH".into());
        }
        Ok(Self { selection_id: selection_id.into(), repository_id: repository_id.into(),
            generation_id: generation_id.into(), canonical_commit: canonical_commit.into(), mode: mode.into() })
    }
}

/// A result retains its complete selection identity; entry IDs remain reader IDs.
pub struct Selected<T> {
    pub binding: Binding,
    pub value: T,
}

trait Session {
    fn generation(&self) -> Option<&str>;
    fn query(&mut self, filters: &Filters, offset: usize) -> Result<Page, String>;
    fn materialize(&mut self, id: &str) -> Result<Value, String>;
}

impl Session for Client {
    fn generation(&self) -> Option<&str> {
        self.generation.as_deref()
    }
    fn query(&mut self, filters: &Filters, offset: usize) -> Result<Page, String> {
        Client::query(self, filters, offset)
    }
    fn materialize(&mut self, id: &str) -> Result<Value, String> {
        Client::materialize(self, id)
    }
}

struct Member {
    binding: Binding,
    cache: PathBuf,
    session: Box<dyn Session>,
}

/// No global aggregation: a failure affects only the operation on that member.
#[derive(Default)]
pub struct Federation {
    members: BTreeMap<String, Member>,
}

impl Federation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start one v1 reader and scan it, accepting it only if its full summary
    /// matches the supplied binding. This may write the selected reader cache.
    /// A failed addition leaves all previously added members intact.
    pub fn add(&mut self, binding: Binding, config: &ReaderConfig) -> Result<Value, String> {
        self.validate_selection(&binding)?;
        let cache = config.cache.as_ref().ok_or("FEDERATION_CACHE_REQUIRED")?;
        if !cache.is_absolute() {
            return Err("FEDERATION_CACHE_MUST_BE_ABSOLUTE".into());
        }
        let cache = cache.canonicalize().map_err(|_| "FEDERATION_CACHE_DIRECTORY_UNAVAILABLE")?;
        if !cache.is_dir() {
            return Err("FEDERATION_CACHE_DIRECTORY_UNAVAILABLE".into());
        }
        if self.cache_overlaps(&cache) {
            return Err("FEDERATION_CACHE_OVERLAP".into());
        }
        let mut config = config.clone();
        config.cache = Some(cache.clone());
        let mut client = Client::connect(&config)?;
        let summary = client.scan(&config, &binding.mode)?;
        self.install(binding, cache, Box::new(client), &summary)?;
        Ok(summary)
    }

    fn cache_overlaps(&self, cache: &std::path::Path) -> bool {
        self.members.values().any(|member| cache.starts_with(&member.cache) || member.cache.starts_with(cache))
    }

    fn validate_selection(&self, binding: &Binding) -> Result<(), String> {
        let valid = |s: &str| !s.is_empty() && s.trim() == s;
        if ![&binding.selection_id, &binding.repository_id, &binding.generation_id, &binding.canonical_commit, &binding.mode]
            .iter().all(|s| valid(s)) || !matches!(binding.mode.as_str(), "committed" | "local") {
            return Err("FEDERATION_BINDING_INVALID".into());
        }
        if self.members.contains_key(&binding.selection_id) || self.members.values().any(|member|
            member.binding.repository_id == binding.repository_id && member.binding.generation_id == binding.generation_id) {
            return Err("FEDERATION_BINDING_DUPLICATE".into());
        }
        Ok(())
    }

    fn install(&mut self, binding: Binding, cache: PathBuf, session: Box<dyn Session>, summary: &Value) -> Result<(), String> {
        self.validate_selection(&binding)?;
        if summary["generationId"].as_str() != Some(binding.generation_id.as_str())
            || summary["snapshot"]["repositoryId"].as_str() != Some(binding.repository_id.as_str())
            || summary["snapshot"]["canonicalCommit"].as_str() != Some(binding.canonical_commit.as_str())
            || summary["snapshot"]["mode"].as_str() != Some(binding.mode.as_str())
            || session.generation() != Some(binding.generation_id.as_str()) {
            return Err("FEDERATION_BINDING_MISMATCH".into());
        }
        self.members.insert(binding.selection_id.clone(), Member { binding, cache, session });
        Ok(())
    }

    pub fn bindings(&self) -> impl Iterator<Item = &Binding> {
        self.members.values().map(|member| &member.binding)
    }

    /// Retire a session. An old binding cannot route to a different replacement.
    pub fn remove(&mut self, selection_id: &str) -> bool {
        self.members.remove(selection_id).is_some()
    }

    fn member(&mut self, binding: &Binding) -> Result<&mut Member, String> {
        let member = self.members.get_mut(&binding.selection_id).ok_or("FEDERATION_SELECTION_UNAVAILABLE")?;
        if member.binding != *binding || member.session.generation() != Some(binding.generation_id.as_str()) {
            return Err("FEDERATION_BINDING_MISMATCH".into());
        }
        Ok(member)
    }

    pub fn query(&mut self, binding: &Binding, filters: &Filters, offset: usize) -> Result<Selected<Page>, String> {
        let member = self.member(binding)?;
        let value = member.session.query(filters, offset)?;
        Ok(Selected { binding: member.binding.clone(), value })
    }

    pub fn materialize(&mut self, binding: &Binding, entry_id: &str) -> Result<Selected<Value>, String> {
        let member = self.member(binding)?;
        let value = member.session.materialize(entry_id)?;
        Ok(Selected { binding: member.binding.clone(), value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Mock { generation: String, name: &'static str, fail: bool }
    impl Session for Mock {
        fn generation(&self) -> Option<&str> { Some(&self.generation) }
        fn query(&mut self, _: &Filters, offset: usize) -> Result<Page, String> {
            if self.fail { return Err("reader failed".into()); }
            Ok(Page { entries: vec![], total: offset + if self.name == "b" { 1 } else { 0 }, next_offset: None })
        }
        fn materialize(&mut self, id: &str) -> Result<Value, String> {
            if self.fail { return Err("reader failed".into()); }
            Ok(json!({"reader": self.name, "id": id}))
        }
    }
    fn binding(id: &str) -> Binding {
        Binding { selection_id: id.into(), repository_id: format!("repo-{id}"), generation_id: format!("gen-{id}"),
            canonical_commit: format!("commit-{id}"), mode: "committed".into() }
    }
    fn summary(b: &Binding) -> Value {
        json!({"generationId": b.generation_id, "snapshot": {"repositoryId": b.repository_id,
            "canonicalCommit": b.canonical_commit, "mode": b.mode}})
    }
    fn install(f: &mut Federation, b: &Binding, name: &'static str, fail: bool) -> Result<(), String> {
        f.install(b.clone(), PathBuf::from(name), Box::new(Mock { generation: b.generation_id.clone(), name, fail }), &summary(b))
    }

    #[test]
    fn routes_identical_entry_ids_to_the_selected_reader_and_isolates_failure() {
        let mut f = Federation::new();
        let (a, b, c) = (binding("a"), binding("b"), binding("c"));
        install(&mut f, &a, "a", false).unwrap();
        install(&mut f, &b, "b", false).unwrap();
        install(&mut f, &c, "c", true).unwrap();
        for selected in [&a, &b] {
            let item = f.materialize(selected, "same-id").unwrap();
            assert_eq!(item.binding, *selected);
            assert_eq!(item.value["reader"], selected.selection_id);
            assert_eq!(item.value["id"], "same-id");
            let page = f.query(selected, &Filters::default(), 7).unwrap();
            assert_eq!(page.binding, *selected);
            assert_eq!(page.value.total, if selected.selection_id == "b" { 8 } else { 7 });
        }
        assert!(f.query(&c, &Filters::default(), 0).is_err());
        assert!(f.materialize(&c, "same-id").is_err());
        assert!(f.materialize(&a, "same-id").is_ok());
    }

    #[test]
    fn rejects_every_mismatched_binding_and_retired_generation() {
        let mut f = Federation::new();
        let a = binding("a");
        install(&mut f, &a, "a", false).unwrap();
        for field in 0..5 {
            let mut wrong = a.clone();
            match field { 0 => wrong.selection_id.push('x'), 1 => wrong.repository_id.push('x'),
                2 => wrong.generation_id.push('x'), 3 => wrong.canonical_commit.push('x'), _ => wrong.mode = "local".into() }
            assert!(f.query(&wrong, &Filters::default(), 0).is_err());
            assert!(f.materialize(&wrong, "same-id").is_err());
        }
        assert!(f.remove("a"));
        let mut newer = a.clone();
        newer.generation_id = "new".into();
        install(&mut f, &newer, "new", false).unwrap();
        assert!(f.materialize(&a, "same-id").is_err());
        assert!(f.materialize(&newer, "same-id").is_ok());
    }

    #[test]
    fn refuses_unverified_summaries_duplicate_bindings_and_session_generation() {
        let mut f = Federation::new();
        let a = binding("a");
        for key in ["generationId", "repositoryId", "canonicalCommit", "mode"] {
            let mut value = summary(&a);
            if key == "generationId" { value[key] = json!("wrong"); } else { value["snapshot"][key] = json!("wrong"); }
            assert!(f.install(a.clone(), "cache".into(), Box::new(Mock { generation: a.generation_id.clone(), name: "a", fail: false }), &value).is_err());
        }
        assert!(f.install(a.clone(), "cache".into(), Box::new(Mock { generation: "wrong".into(), name: "a", fail: false }), &summary(&a)).is_err());
        install(&mut f, &a, "a", false).unwrap();
        assert!(f.cache_overlaps(std::path::Path::new("a")));
        assert!(f.cache_overlaps(&PathBuf::from("a").join("nested")));
        assert!(!f.cache_overlaps(std::path::Path::new("b")));
        assert!(install(&mut f, &a, "a", false).is_err());
        let mut duplicate = a.clone();
        duplicate.selection_id = "another".into();
        assert!(install(&mut f, &duplicate, "b", false).is_err());
        assert_eq!(f.bindings().count(), 1);
        f.members.get_mut("a").unwrap().session = Box::new(Mock { generation: "changed".into(), name: "a", fail: false });
        assert!(f.query(&a, &Filters::default(), 0).is_err());
        assert!(f.materialize(&a, "same-id").is_err());
    }

    #[test]
    fn binds_a_first_scan_to_selection_and_rejects_mismatched_or_malformed_identity() {
        let repository = "a".repeat(64);
        let generation = "b".repeat(64);
        let commit = "c".repeat(40);
        let observed = json!({"generationId": generation, "snapshot": {
            "repositoryId": repository, "canonicalCommit": commit, "mode": "committed"}});
        let binding = Binding::from_summary("cms-main", "committed", &observed, Some(&repository), Some(&commit)).unwrap();
        assert_eq!(binding.generation_id, generation);
        assert!(Binding::from_summary("cms-main", "local", &observed, None, None).is_err());
        assert!(Binding::from_summary("cms-main", "committed", &observed, Some(&"d".repeat(64)), None).is_err());
        assert!(Binding::from_summary("cms-main", "committed", &observed, None, Some(&"e".repeat(40))).is_err());
        assert!(Binding::from_summary("../cms", "committed", &observed, None, None).is_err());
        let mut malformed = observed;
        malformed["generationId"] = json!("not-a-digest");
        assert!(Binding::from_summary("cms-main", "committed", &malformed, None, None).is_err());
    }
}
