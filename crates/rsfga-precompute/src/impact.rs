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

/// Build a map of relation dependencies from type definitions, with transitive closure.
///
/// Input format: type_name -> relation_name -> [referenced_relations]
/// For each (type, relation) pair, returns the set of other (type, relation) pairs
/// whose check results could be affected when tuples for this (type, relation) change.
///
/// Example: If `viewer` references `editor` and `editor` references `owner`
/// (i.e., `define viewer: [user] or editor; define editor: [user] or owner`),
/// then changes to `owner` tuples affect both `editor` AND `viewer` checks.
/// So `deps[("document", "owner")]` includes both `("document", "editor")` and
/// `("document", "viewer")`.
///
/// The transitive closure ensures chains like `viewer → editor → owner` are
/// fully expanded: a change to `owner` propagates through `editor` to `viewer`.
#[must_use]
pub fn build_relation_dependencies(
    type_definitions: &HashMap<String, HashMap<String, Vec<String>>>,
) -> HashMap<(String, String), HashSet<(String, String)>> {
    // Step 1: Build single-hop (direct) dependencies
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

    // Step 2: Compute transitive closure via fixed-point iteration.
    // If A affects B and B affects C, then A also affects C.
    //
    // The iteration limit is N (number of unique relation keys). In the worst case
    // (a linear chain of N relations), each iteration propagates dependencies one
    // hop further, so N iterations suffice for full transitive closure. If we
    // exceed this, the graph has a cycle or a bug — log a warning and return
    // the partial result rather than looping indefinitely.
    let max_iterations = deps.len().max(1);
    for iteration in 0..max_iterations {
        let mut changed = false;
        let keys: Vec<_> = deps.keys().cloned().collect();

        for key in &keys {
            // For each dependent of `key`, check if that dependent has its own dependents
            let dependents: Vec<_> = deps
                .get(key)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect();
            for dependent in &dependents {
                if let Some(transitive) = deps.get(dependent).cloned() {
                    let entry = deps.entry(key.clone()).or_default();
                    for t in transitive {
                        if entry.insert(t) {
                            changed = true;
                        }
                    }
                }
            }
        }

        if !changed {
            break;
        }

        if iteration == max_iterations - 1 {
            tracing::warn!(
                iterations = max_iterations,
                "Transitive closure did not converge within iteration limit; \
                 possible cycle in relation definitions"
            );
        }
    }

    deps
}

/// Determine which hot-path checks need recomputation based on classified changes.
#[must_use = "jobs should be submitted to the worker pool"]
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
    fn test_build_relation_dependencies_chain_transitive() {
        // viewer → editor → owner (3-level chain)
        let mut type_defs: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
        let mut doc_relations = HashMap::new();
        doc_relations.insert("viewer".to_string(), vec!["editor".to_string()]);
        doc_relations.insert("editor".to_string(), vec!["owner".to_string()]);
        doc_relations.insert("owner".to_string(), vec![]);
        type_defs.insert("document".to_string(), doc_relations);

        let deps = build_relation_dependencies(&type_defs);

        // Direct: owner change → recompute editor
        assert!(deps[&("document".to_string(), "owner".to_string())]
            .contains(&("document".to_string(), "editor".to_string())));
        // Direct: editor change → recompute viewer
        assert!(deps[&("document".to_string(), "editor".to_string())]
            .contains(&("document".to_string(), "viewer".to_string())));
        // Transitive: owner change → recompute viewer (through editor)
        assert!(
            deps[&("document".to_string(), "owner".to_string())]
                .contains(&("document".to_string(), "viewer".to_string())),
            "Transitive closure missing: owner → viewer"
        );
    }

    #[test]
    fn test_build_relation_dependencies_deep_chain() {
        // a → b → c → d (4-level chain)
        let mut type_defs: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
        let mut rels = HashMap::new();
        rels.insert("a".to_string(), vec!["b".to_string()]);
        rels.insert("b".to_string(), vec!["c".to_string()]);
        rels.insert("c".to_string(), vec!["d".to_string()]);
        rels.insert("d".to_string(), vec![]);
        type_defs.insert("t".to_string(), rels);

        let deps = build_relation_dependencies(&type_defs);

        // d change should affect a, b, c (transitive)
        let d_deps = &deps[&("t".to_string(), "d".to_string())];
        assert!(d_deps.contains(&("t".to_string(), "c".to_string())));
        assert!(d_deps.contains(&("t".to_string(), "b".to_string())));
        assert!(d_deps.contains(&("t".to_string(), "a".to_string())));
    }

    #[test]
    fn test_build_relation_dependencies_diamond() {
        // viewer → editor, viewer → commenter, editor → owner, commenter → owner
        let mut type_defs: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
        let mut rels = HashMap::new();
        rels.insert(
            "viewer".to_string(),
            vec!["editor".to_string(), "commenter".to_string()],
        );
        rels.insert("editor".to_string(), vec!["owner".to_string()]);
        rels.insert("commenter".to_string(), vec!["owner".to_string()]);
        rels.insert("owner".to_string(), vec![]);
        type_defs.insert("doc".to_string(), rels);

        let deps = build_relation_dependencies(&type_defs);

        // owner change → affects editor, commenter, AND viewer (transitive)
        let owner_deps = &deps[&("doc".to_string(), "owner".to_string())];
        assert!(owner_deps.contains(&("doc".to_string(), "editor".to_string())));
        assert!(owner_deps.contains(&("doc".to_string(), "commenter".to_string())));
        assert!(owner_deps.contains(&("doc".to_string(), "viewer".to_string())));
    }

    #[test]
    fn test_build_relation_dependencies_empty() {
        let type_defs: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
        let deps = build_relation_dependencies(&type_defs);
        assert!(deps.is_empty());
    }

    #[test]
    fn test_build_relation_dependencies_cycle_terminates() {
        // a → b → a (cycle) — must not loop forever
        let mut type_defs: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
        let mut rels = HashMap::new();
        rels.insert("a".to_string(), vec!["b".to_string()]);
        rels.insert("b".to_string(), vec!["a".to_string()]);
        type_defs.insert("t".to_string(), rels);

        // Should complete without hanging
        let deps = build_relation_dependencies(&type_defs);

        // Both directions should exist due to the cycle
        assert!(
            deps[&("t".to_string(), "a".to_string())].contains(&("t".to_string(), "b".to_string()))
        );
        assert!(
            deps[&("t".to_string(), "b".to_string())].contains(&("t".to_string(), "a".to_string()))
        );
    }
}
