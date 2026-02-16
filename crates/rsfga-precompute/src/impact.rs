//! Impact analysis: determine which hot-path checks need recomputation.

use std::collections::{HashMap, HashSet};

use rsfga_valkey::cache::CheckCache;
use rsfga_valkey::keys;
use tracing::debug;

use crate::classifier::ChangeType;
use crate::error::Result;

/// A job to recompute a specific check result.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct RecomputeJob {
    pub store_id: String,
    pub object_type: String,
    pub object_id: String,
    pub relation: String,
    pub user: String,
}

/// Build a map of relation dependencies from type definitions.
///
/// Input format: type_name -> relation_name -> [referenced_relations]
/// For each (type, relation) pair, returns the set of other (type, relation) pairs
/// whose check results could be affected when tuples for this (type, relation) change.
///
/// Example: If `viewer` references `editor` (i.e., `define viewer: [user] or editor`),
/// then changes to `editor` tuples affect `viewer` checks.
/// So `deps[("document", "editor")]` includes `("document", "viewer")`.
///
/// TODO: Compute the transitive closure of the dependency graph. Currently only
/// single-hop dependencies are captured: in a chain `viewer → editor → owner`,
/// changing an `owner` tuple creates jobs for `editor` but not `viewer`.
pub fn build_relation_dependencies(
    type_definitions: &HashMap<String, HashMap<String, Vec<String>>>,
) -> HashMap<(String, String), HashSet<(String, String)>> {
    let mut deps: HashMap<(String, String), HashSet<(String, String)>> = HashMap::new();

    for (type_name, relations) in type_definitions {
        for (relation_name, referenced_relations) in relations {
            for ref_rel in referenced_relations {
                deps.entry((type_name.clone(), ref_rel.clone()))
                    .or_default()
                    .insert((type_name.clone(), relation_name.clone()));
            }
        }
    }

    deps
}

/// Determine which hot-path checks need recomputation based on classified changes.
pub async fn find_affected_checks(
    changes: &[ChangeType],
    cache: &CheckCache,
    relation_deps: &HashMap<(String, String), HashSet<(String, String)>>,
) -> Result<Vec<RecomputeJob>> {
    let mut jobs = HashSet::new();

    for change in changes {
        match change {
            ChangeType::TupleChange {
                store_id,
                object_type,
                relation,
            } => {
                // Direct: find hot-path entries for this (object_type, relation)
                let pattern = keys::hotpath_pattern(object_type, relation);
                let members = cache.scan_hotpath(store_id, &pattern).await?;
                for member in &members {
                    if let Some(job) = make_recompute_job(store_id, member) {
                        jobs.insert(job);
                    }
                }

                // Indirect: find computed relations that depend on this relation
                let key = (object_type.clone(), relation.clone());
                if let Some(dependents) = relation_deps.get(&key) {
                    for (dep_type, dep_rel) in dependents {
                        let dep_pattern = keys::hotpath_pattern(dep_type, dep_rel);
                        let dep_members = cache.scan_hotpath(store_id, &dep_pattern).await?;
                        for member in &dep_members {
                            if let Some(job) = make_recompute_job(store_id, member) {
                                jobs.insert(job);
                            }
                        }
                    }
                }

                debug!(
                    store_id,
                    object_type,
                    relation,
                    job_count = jobs.len(),
                    "Found affected checks for tuple change"
                );
            }
            ChangeType::ModelChange { store_id } => {
                let all_members = cache.get_all_hotpath(store_id).await?;
                for member in &all_members {
                    if let Some(job) = make_recompute_job(store_id, member) {
                        jobs.insert(job);
                    }
                }

                debug!(
                    store_id,
                    job_count = jobs.len(),
                    "Found affected checks for model change"
                );
            }
        }
    }

    Ok(jobs.into_iter().collect())
}

/// Parse a hot-path member string into a RecomputeJob.
/// Delegates to `keys::parse_hotpath_member` which handles percent-encoded delimiters.
fn make_recompute_job(store_id: &str, member: &str) -> Option<RecomputeJob> {
    let (object_type, object_id, relation, user) = keys::parse_hotpath_member(member)?;
    Some(RecomputeJob {
        store_id: store_id.to_string(),
        object_type,
        object_id,
        relation,
        user,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_recompute_job_valid() {
        let job = make_recompute_job("store1", "document:readme#viewer@user:alice").unwrap();
        assert_eq!(job.store_id, "store1");
        assert_eq!(job.object_type, "document");
        assert_eq!(job.object_id, "readme");
        assert_eq!(job.relation, "viewer");
        assert_eq!(job.user, "user:alice");
    }

    #[test]
    fn test_make_recompute_job_with_encoded_chars() {
        let member = keys::hotpath_member("document", "my#doc", "viewer", "group:eng#member");
        let job = make_recompute_job("store1", &member).unwrap();
        assert_eq!(job.object_id, "my#doc");
        assert_eq!(job.user, "group:eng#member");
    }

    #[test]
    fn test_make_recompute_job_invalid() {
        assert!(make_recompute_job("s", "invalid").is_none());
        assert!(make_recompute_job("s", "no_hash@user").is_none());
        assert!(make_recompute_job("s", "type:id#no_at").is_none());
    }

    #[test]
    fn test_build_relation_dependencies_simple() {
        let mut type_defs: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
        let mut doc_relations = HashMap::new();
        doc_relations.insert("viewer".to_string(), vec!["editor".to_string()]);
        doc_relations.insert("editor".to_string(), vec![]);
        type_defs.insert("document".to_string(), doc_relations);

        let deps = build_relation_dependencies(&type_defs);

        let affected = deps.get(&("document".to_string(), "editor".to_string()));
        assert!(affected.is_some());
        assert!(affected
            .unwrap()
            .contains(&("document".to_string(), "viewer".to_string())));

        assert!(!deps.contains_key(&("document".to_string(), "viewer".to_string())));
    }

    #[test]
    fn test_build_relation_dependencies_chain() {
        let mut type_defs: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
        let mut doc_relations = HashMap::new();
        doc_relations.insert("viewer".to_string(), vec!["editor".to_string()]);
        doc_relations.insert("editor".to_string(), vec!["owner".to_string()]);
        doc_relations.insert("owner".to_string(), vec![]);
        type_defs.insert("document".to_string(), doc_relations);

        let deps = build_relation_dependencies(&type_defs);

        assert!(deps[&("document".to_string(), "owner".to_string())]
            .contains(&("document".to_string(), "editor".to_string())));
        assert!(deps[&("document".to_string(), "editor".to_string())]
            .contains(&("document".to_string(), "viewer".to_string())));
    }

    #[test]
    fn test_build_relation_dependencies_empty() {
        let type_defs: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
        let deps = build_relation_dependencies(&type_defs);
        assert!(deps.is_empty());
    }
}
