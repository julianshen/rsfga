//! Property-based tests for model types.

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    /// Strategy to generate valid user identifiers in type:id format
    fn valid_user_strategy() -> impl Strategy<Value = String> {
        // Generate type:id format like "user:alice" or "group:engineers"
        ("[a-z]{1,10}", "[a-z0-9]{1,20}").prop_map(|(t, id)| format!("{t}:{id}"))
    }

    /// Strategy to generate valid userset references in type:id#relation format
    fn userset_reference_strategy() -> impl Strategy<Value = String> {
        ("[a-z]{1,10}", "[a-z0-9]{1,10}", "[a-z]{1,10}")
            .prop_map(|(t, id, rel)| format!("{t}:{id}#{rel}"))
    }

    proptest! {
        #[test]
        fn test_user_type_id_format_is_valid(user_str in valid_user_strategy()) {
            use crate::model::User;
            // Any valid type:id format should create a valid User
            let user = User::new(&user_str);
            prop_assert!(user.is_ok(), "Failed for user: {}", user_str);
            let user = user.unwrap();
            prop_assert_eq!(user.as_str(), user_str);
        }

        #[test]
        fn test_user_userset_reference_is_valid(user_str in userset_reference_strategy()) {
            use crate::model::User;
            // Userset references should be valid
            let user = User::new(&user_str);
            prop_assert!(user.is_ok(), "Failed for userset: {}", user_str);
            let user = user.unwrap();
            prop_assert!(user.is_userset_reference());
        }

        #[test]
        fn test_user_without_colon_is_invalid(s in "[a-z]{1,20}") {
            use crate::model::User;
            // Strings without colon should be rejected (unless *)
            if s != "*" {
                let user = User::new(&s);
                prop_assert!(user.is_err(), "Should reject: {}", s);
            }
        }

        #[test]
        fn test_object_parse_roundtrip(
            obj_type in "[a-z]{1,10}",
            obj_id in "[a-z0-9]{1,10}"
        ) {
            use crate::model::Object;
            let input = format!("{obj_type}:{obj_id}");
            let parsed = Object::parse(&input);
            prop_assert!(parsed.is_ok());
            let obj = parsed.unwrap();
            prop_assert_eq!(obj.to_string(), input);
        }

        #[test]
        fn test_tuple_valid_fields_succeed(
            user_type in "[a-z]{1,10}",
            user_id in "[a-z0-9]{1,10}",
            relation in "[a-z]{1,10}",
            object in "[a-z0-9:]{3,20}"
        ) {
            use crate::model::Tuple;
            let user = format!("{user_type}:{user_id}");
            // Non-empty strings for all fields should succeed
            if !relation.is_empty() && !object.is_empty() {
                let tuple = Tuple::new(&user, &relation, &object);
                prop_assert!(tuple.is_ok());
            }
        }

        #[test]
        fn test_store_valid_id_and_name_succeed(
            id in "[a-z0-9-]{1,36}",
            name in "[a-zA-Z0-9 ]{1,50}"
        ) {
            use crate::model::Store;
            // Non-empty id and name should succeed
            if !id.is_empty() && !name.is_empty() {
                let store = Store::new(&id, &name);
                prop_assert!(store.is_ok());
                let store = store.unwrap();
                prop_assert_eq!(store.id, id);
                prop_assert_eq!(store.name, name);
            }
        }
    }
}
