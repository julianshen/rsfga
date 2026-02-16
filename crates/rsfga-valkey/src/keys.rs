/// Percent-encode delimiter characters (#, @, %) in field values.
///
/// OpenFGA object IDs can contain any non-whitespace character, including
/// `#` and `@` which we use as delimiters in Redis keys. Encoding prevents
/// delimiter collision that could cause cache poisoning.
fn encode_field(s: &str) -> String {
    s.replace('%', "%25")
        .replace('#', "%23")
        .replace('@', "%40")
}

/// Decode percent-encoded field values.
fn decode_field(s: &str) -> String {
    s.replace("%23", "#")
        .replace("%40", "@")
        .replace("%25", "%")
}

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
            encode_field(&self.object_id),
            self.relation,
            encode_field(&self.user),
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
            object_id: decode_field(object_id),
            relation: relation.to_string(),
            user: decode_field(user),
        })
    }
}

pub fn hotpath_key(store_id: &str) -> String {
    format!("hotpath:{store_id}")
}

pub fn hotpath_member(object_type: &str, object_id: &str, relation: &str, user: &str) -> String {
    format!(
        "{}:{}#{}@{}",
        object_type,
        encode_field(object_id),
        relation,
        encode_field(user),
    )
}

/// Parse a hot-path member string into its components.
/// Format: `{object_type}:{encoded_object_id}#{relation}@{encoded_user}`
pub fn parse_hotpath_member(member: &str) -> Option<(String, String, String, String)> {
    let colon_pos = member.find(':')?;
    let object_type = &member[..colon_pos];
    let after_colon = &member[colon_pos + 1..];

    let hash_pos = after_colon.find('#')?;
    let object_id = &after_colon[..hash_pos];
    let after_hash = &after_colon[hash_pos + 1..];

    let at_pos = after_hash.find('@')?;
    let relation = &after_hash[..at_pos];
    let user = &after_hash[at_pos + 1..];

    Some((
        object_type.to_string(),
        decode_field(object_id),
        relation.to_string(),
        decode_field(user),
    ))
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
    fn test_encode_decode_roundtrip() {
        let cases = [
            "simple",
            "with#hash",
            "with@at",
            "with%percent",
            "all#three@chars%here",
            "my#doc#multiple##hashes",
            "user@example.com",
        ];
        for case in cases {
            assert_eq!(
                decode_field(&encode_field(case)),
                case,
                "failed for: {case}"
            );
        }
    }

    #[test]
    fn test_encode_field_values() {
        assert_eq!(encode_field("simple"), "simple");
        assert_eq!(encode_field("my#doc"), "my%23doc");
        assert_eq!(encode_field("user@ex.com"), "user%40ex.com");
        assert_eq!(encode_field("100%"), "100%25");
        assert_eq!(encode_field("a#b@c%d"), "a%23b%40c%25d");
    }

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
    fn test_check_key_with_hash_in_object_id() {
        let key = CheckKey::new(
            "store1",
            "model1",
            "document",
            "my#doc",
            "viewer",
            "user:alice",
        );
        let redis_key = key.to_redis_key();
        assert_eq!(
            redis_key,
            "check:store1:model1:document:my%23doc#viewer@user:alice"
        );
        let parsed = CheckKey::from_redis_key(&redis_key).unwrap();
        assert_eq!(parsed, key);
    }

    #[test]
    fn test_check_key_with_at_in_user() {
        let key = CheckKey::new(
            "store1",
            "model1",
            "document",
            "readme",
            "viewer",
            "user:bob@example.com",
        );
        let redis_key = key.to_redis_key();
        let parsed = CheckKey::from_redis_key(&redis_key).unwrap();
        assert_eq!(parsed, key);
    }

    #[test]
    fn test_check_key_with_userset_user() {
        let key = CheckKey::new(
            "store1",
            "model1",
            "document",
            "readme",
            "viewer",
            "group:eng#member",
        );
        let redis_key = key.to_redis_key();
        let parsed = CheckKey::from_redis_key(&redis_key).unwrap();
        assert_eq!(parsed, key);
    }

    #[test]
    fn test_check_key_with_percent_in_object_id() {
        let key = CheckKey::new(
            "store1",
            "model1",
            "document",
            "100%done",
            "viewer",
            "user:alice",
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
    fn test_hotpath_member_with_special_chars() {
        let member = hotpath_member("document", "my#doc", "viewer", "group:eng#member");
        assert_eq!(member, "document:my%23doc#viewer@group:eng%23member");
    }

    #[test]
    fn test_parse_hotpath_member_roundtrip() {
        let cases = [
            ("document", "readme", "viewer", "user:alice"),
            ("document", "my#doc", "viewer", "user:alice"),
            ("folder", "path@host", "owner", "group:eng#member"),
            ("item", "100%done", "editor", "user:bob@example.com"),
        ];
        for (obj_type, obj_id, rel, user) in cases {
            let member = hotpath_member(obj_type, obj_id, rel, user);
            let (pt, pi, pr, pu) = parse_hotpath_member(&member).unwrap();
            assert_eq!(pt, obj_type);
            assert_eq!(pi, obj_id);
            assert_eq!(pr, rel);
            assert_eq!(pu, user);
        }
    }

    #[test]
    fn test_parse_hotpath_member_invalid() {
        assert!(parse_hotpath_member("invalid").is_none());
        assert!(parse_hotpath_member("no_hash@user").is_none());
        assert!(parse_hotpath_member("type:id#no_at").is_none());
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
