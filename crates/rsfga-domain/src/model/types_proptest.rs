//! Property-based tests for model types.

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn test_user_nonempty_is_valid(s in ".+") {
            use crate::model::User;
            // Any non-empty string should create a valid User
            let user = User::new(s);
            prop_assert!(user.is_ok());
        }

        #[test]
        fn test_object_parse_roundtrip(
            obj_type in "[a-z]{1,10}",
            obj_id in "[a-z0-9]{1,10}"
        ) {
            use crate::model::Object;
            let input = format!("{}:{}", obj_type, obj_id);
            let parsed = Object::parse(&input);
            prop_assert!(parsed.is_ok());
            let obj = parsed.unwrap();
            prop_assert_eq!(obj.to_string(), input);
        }
    }
}
