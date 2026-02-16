use rsfga_nats::CommittedEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeType {
    TupleChange {
        store_id: String,
        object_type: String,
        relation: String,
    },
    ModelChange {
        store_id: String,
    },
}

/// Classify a committed event into change types for impact analysis.
pub fn classify(event: &CommittedEvent) -> Vec<ChangeType> {
    let mut seen = std::collections::HashSet::new();
    let mut changes = Vec::new();

    // Extract unique (object_type, relation) pairs from writes
    for write in &event.writes {
        if let Some((obj_type, _)) = write.key.object.split_once(':') {
            let pair = (obj_type.to_string(), write.key.relation.clone());
            if seen.insert(pair.clone()) {
                changes.push(ChangeType::TupleChange {
                    store_id: event.store_id.clone(),
                    object_type: pair.0,
                    relation: pair.1,
                });
            }
        }
    }

    // Extract unique (object_type, relation) pairs from deletes
    for delete in &event.deletes {
        if let Some((obj_type, _)) = delete.object.split_once(':') {
            let pair = (obj_type.to_string(), delete.relation.clone());
            if seen.insert(pair.clone()) {
                changes.push(ChangeType::TupleChange {
                    store_id: event.store_id.clone(),
                    object_type: pair.0,
                    relation: pair.1,
                });
            }
        }
    }

    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsfga_nats::{CommittedEvent, TupleKey, TupleOperation};

    fn make_event(writes: Vec<TupleOperation>, deletes: Vec<TupleKey>) -> CommittedEvent {
        CommittedEvent::new("store1", 1)
            .with_writes(writes)
            .with_deletes(deletes)
    }

    fn tuple_op(user: &str, relation: &str, object: &str) -> TupleOperation {
        TupleOperation::new(user, relation, object)
    }

    fn tuple_key(user: &str, relation: &str, object: &str) -> TupleKey {
        TupleKey {
            user: user.to_string(),
            relation: relation.to_string(),
            object: object.to_string(),
        }
    }

    #[test]
    fn test_classify_single_tuple_write() {
        let event = make_event(
            vec![tuple_op("user:alice", "viewer", "document:readme")],
            vec![],
        );
        let changes = classify(&event);
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0],
            ChangeType::TupleChange {
                store_id: "store1".to_string(),
                object_type: "document".to_string(),
                relation: "viewer".to_string(),
            }
        );
    }

    #[test]
    fn test_classify_tuple_delete() {
        let event = make_event(
            vec![],
            vec![tuple_key("user:alice", "viewer", "document:readme")],
        );
        let changes = classify(&event);
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0],
            ChangeType::TupleChange {
                store_id: "store1".to_string(),
                object_type: "document".to_string(),
                relation: "viewer".to_string(),
            }
        );
    }

    #[test]
    fn test_classify_deduplicates_same_type_relation() {
        let event = make_event(
            vec![
                tuple_op("user:alice", "viewer", "document:doc1"),
                tuple_op("user:bob", "viewer", "document:doc2"),
            ],
            vec![],
        );
        let changes = classify(&event);
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn test_classify_different_relations_not_deduped() {
        let event = make_event(
            vec![
                tuple_op("user:alice", "viewer", "document:doc1"),
                tuple_op("user:alice", "editor", "document:doc1"),
            ],
            vec![],
        );
        let changes = classify(&event);
        assert_eq!(changes.len(), 2);
    }

    #[test]
    fn test_classify_empty_event() {
        let event = make_event(vec![], vec![]);
        let changes = classify(&event);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_classify_invalid_object_format_skipped() {
        let event = CommittedEvent::new("store1", 1).with_writes(vec![TupleOperation::new(
            "user:alice",
            "viewer",
            "invalidobject", // No type:id separator
        )]);
        let changes = classify(&event);
        assert!(changes.is_empty());
    }
}
