#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CheckKey {
    pub store_id: String,
    pub model_id: String,
    pub object_type: String,
    pub object_id: String,
    pub relation: String,
    pub user: String,
}

impl CheckKey {
    pub fn new(
        store_id: impl Into<String>,
        model_id: impl Into<String>,
        object_type: impl Into<String>,
        object_id: impl Into<String>,
        relation: impl Into<String>,
        user: impl Into<String>,
    ) -> Self {
        Self {
            store_id: store_id.into(),
            model_id: model_id.into(),
            object_type: object_type.into(),
            object_id: object_id.into(),
            relation: relation.into(),
            user: user.into(),
        }
    }

    pub fn to_redis_key(&self) -> String {
        format!(
            "check:{}:{}:{}:{}#{}@{}",
            self.store_id,
            self.model_id,
            self.object_type,
            self.object_id,
            self.relation,
            self.user,
        )
    }

    pub fn from_redis_key(key: &str) -> Option<Self> {
        let rest = key.strip_prefix("check:")?;
        let mut parts = rest.splitn(4, ':');
        let store_id = parts.next()?;
        let model_id = parts.next()?;
        let object_type = parts.next()?;
        let remainder = parts.next()?;
        let hash_pos = remainder.find('#')?;
        let object_id = &remainder[..hash_pos];
        let after_hash = &remainder[hash_pos + 1..];
        let at_pos = after_hash.find('@')?;
        let relation = &after_hash[..at_pos];
        let user = &after_hash[at_pos + 1..];
        Some(Self {
            store_id: store_id.to_string(),
            model_id: model_id.to_string(),
            object_type: object_type.to_string(),
            object_id: object_id.to_string(),
            relation: relation.to_string(),
            user: user.to_string(),
        })
    }
}

pub fn hotpath_key(store_id: &str) -> String {
    format!("hotpath:{store_id}")
}

pub fn hotpath_member(object_type: &str, object_id: &str, relation: &str, user: &str) -> String {
    format!("{object_type}:{object_id}#{relation}@{user}")
}

pub fn hotpath_pattern(object_type: &str, relation: &str) -> String {
    format!("{object_type}:*#{relation}@*")
}

pub fn store_check_prefix(store_id: &str) -> String {
    format!("check:{store_id}:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_key_roundtrip() {
        let key = CheckKey::new(
            "store1",
            "model1",
            "document",
            "readme",
            "viewer",
            "user:alice",
        );
        let redis_key = key.to_redis_key();
        assert_eq!(
            redis_key,
            "check:store1:model1:document:readme#viewer@user:alice"
        );
        let parsed = CheckKey::from_redis_key(&redis_key).unwrap();
        assert_eq!(parsed, key);
    }

    #[test]
    fn test_check_key_with_special_characters() {
        let key = CheckKey::new(
            "store1",
            "model1",
            "document",
            "doc-with-dashes",
            "can_view",
            "user:bob",
        );
        let redis_key = key.to_redis_key();
        let parsed = CheckKey::from_redis_key(&redis_key).unwrap();
        assert_eq!(parsed, key);
    }

    #[test]
    fn test_check_key_from_invalid_format() {
        assert!(CheckKey::from_redis_key("invalid").is_none());
        assert!(CheckKey::from_redis_key("check:").is_none());
        assert!(CheckKey::from_redis_key("check:s:m:t:obj").is_none());
        assert!(CheckKey::from_redis_key("wrong_prefix:s:m:t:o#r@u").is_none());
    }

    #[test]
    fn test_hotpath_key_format() {
        assert_eq!(hotpath_key("store123"), "hotpath:store123");
    }

    #[test]
    fn test_hotpath_member_format() {
        let member = hotpath_member("document", "readme", "viewer", "user:alice");
        assert_eq!(member, "document:readme#viewer@user:alice");
    }

    #[test]
    fn test_hotpath_pattern_format() {
        let pattern = hotpath_pattern("document", "viewer");
        assert_eq!(pattern, "document:*#viewer@*");
    }

    #[test]
    fn test_store_check_prefix() {
        assert_eq!(store_check_prefix("store1"), "check:store1:");
    }
}
