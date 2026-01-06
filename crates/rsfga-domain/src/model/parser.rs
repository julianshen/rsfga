//! DSL parser for OpenFGA authorization models.
//!
//! Parses the OpenFGA DSL format into AuthorizationModel structures.
//!
//! Example DSL:
//! ```text
//! type user
//!
//! type document
//!   relations
//!     define owner: [user]
//!     define editor: [user] or owner
//!     define viewer: [user] or editor
//! ```

use nom::{
    branch::alt,
    bytes::complete::{tag, take_while, take_while1},
    character::complete::{char, multispace1, space0, space1},
    combinator::{all_consuming, map, opt, recognize, value},
    error::{context, ContextError, ParseError},
    multi::{many0, separated_list1},
    sequence::{delimited, pair, preceded, terminated, tuple},
    IResult,
};

use super::{AuthorizationModel, RelationDefinition, TypeDefinition, Userset};

/// Parser error type with context for better error messages.
#[derive(Debug, Clone, PartialEq)]
pub struct ParserError {
    pub message: String,
    pub position: Option<usize>,
}

impl ParserError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            position: None,
        }
    }

    pub fn with_position(message: impl Into<String>, position: usize) -> Self {
        Self {
            message: message.into(),
            position: Some(position),
        }
    }
}

impl std::fmt::Display for ParserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(pos) = self.position {
            write!(f, "{} at position {}", self.message, pos)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl std::error::Error for ParserError {}

/// Result type for parser operations.
pub type ParserResult<T> = Result<T, ParserError>;

// ============ Helper Parsers ============

/// Parse a comment (# to end of line)
fn comment<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, (), E> {
    value((), pair(char('#'), take_while(|c| c != '\n' && c != '\r')))(input)
}

/// Parse whitespace including comments
fn ws<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, (), E> {
    value((), many0(alt((value((), multispace1), comment))))(input)
}

/// Reserved keywords that cannot be used as identifiers
const RESERVED_KEYWORDS: &[&str] = &[
    "type",
    "relations",
    "define",
    "or",
    "and",
    "but",
    "not",
    "from",
    "this",
];

/// Check if a string is a reserved keyword
fn is_reserved(s: &str) -> bool {
    RESERVED_KEYWORDS.contains(&s)
}

/// Parse an identifier (alphanumeric and underscore, not a reserved keyword)
fn identifier<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, &'a str, E> {
    let (rest, id) = take_while1(|c: char| c.is_alphanumeric() || c == '_')(input)?;

    // Reject reserved keywords as identifiers
    if is_reserved(id) {
        return Err(nom::Err::Error(E::from_error_kind(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }

    Ok((rest, id))
}

// ============ Keyword Parsers ============

/// Parse the "type" keyword
fn type_keyword<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, &'a str, E> {
    context("type keyword", tag("type"))(input)
}

/// Parse the "relations" keyword
fn relations_keyword<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, &'a str, E> {
    context("relations keyword", tag("relations"))(input)
}

/// Parse the "define" keyword
fn define_keyword<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, &'a str, E> {
    context("define keyword", tag("define"))(input)
}

// ============ Type Constraint Parsers ============

/// Parse a type constraint like [user] or [user, group#member]
fn type_constraint<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, Vec<String>, E> {
    context(
        "type constraint",
        delimited(
            char('['),
            separated_list1(
                tuple((space0, char(','), space0)),
                map(
                    recognize(pair(identifier, opt(pair(char('#'), identifier)))),
                    |s: &str| s.to_string(),
                ),
            ),
            char(']'),
        ),
    )(input)
}

// ============ Userset Parsers ============

/// Parse "this" keyword to Userset::This
fn parse_this<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, Userset, E> {
    value(Userset::This, tag("this"))(input)
}

/// Parse a direct relation reference (just a relation name)
fn parse_computed_userset<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, Userset, E> {
    map(identifier, |name: &str| Userset::ComputedUserset {
        relation: name.to_string(),
    })(input)
}

/// Parse "relation from tupleset" (tuple to userset)
fn parse_tuple_to_userset<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, Userset, E> {
    context(
        "tuple to userset",
        map(
            tuple((identifier, space1, tag("from"), space1, identifier)),
            |(computed, _, _, _, tupleset): (&str, _, _, _, &str)| Userset::TupleToUserset {
                tupleset: tupleset.to_string(),
                computed_userset: computed.to_string(),
            },
        ),
    )(input)
}

/// Parse a base userset (this, computed, or tuple_to_userset)
fn parse_base_userset<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, Userset, E> {
    alt((parse_this, parse_tuple_to_userset, parse_computed_userset))(input)
}

/// Parse a userset with "but not" exclusion (highest precedence after base)
fn parse_exclusion_or_base<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, Userset, E> {
    // Try to parse base "but not" subtract
    let exclusion_result: IResult<&str, Userset, E> = context(
        "exclusion",
        map(
            tuple((
                parse_base_userset,
                space1,
                tag("but"),
                space1,
                tag("not"),
                space1,
                parse_base_userset,
            )),
            |(base, _, _, _, _, _, subtract)| Userset::Exclusion {
                base: Box::new(base),
                subtract: Box::new(subtract),
            },
        ),
    )(input);

    match exclusion_result {
        Ok(result) => Ok(result),
        Err(_) => parse_base_userset(input),
    }
}

/// Parse intersection level (and binds tighter than or)
fn parse_intersection_level<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, Userset, E> {
    let (rest, first) = parse_exclusion_or_base(input)?;

    let (rest, and_operands) = many0(preceded(
        tuple((space0, tag("and"), space1)),
        parse_exclusion_or_base,
    ))(rest)?;

    if and_operands.is_empty() {
        Ok((rest, first))
    } else {
        let mut children = vec![first];
        children.extend(and_operands);
        Ok((rest, Userset::Intersection { children }))
    }
}

/// Parse union level (lowest precedence)
fn parse_union_level<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, Userset, E> {
    let (rest, first) = parse_intersection_level(input)?;

    let (rest, or_operands) = many0(preceded(
        tuple((space0, tag("or"), space1)),
        parse_intersection_level,
    ))(rest)?;

    if or_operands.is_empty() {
        Ok((rest, first))
    } else {
        let mut children = vec![first];
        children.extend(or_operands);
        Ok((rest, Userset::Union { children }))
    }
}

/// Parse a complete userset expression with proper operator precedence
/// Precedence (highest to lowest): exclusion (but not), intersection (and), union (or)
fn parse_userset<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, Userset, E> {
    parse_union_level(input)
}

/// Represents the result of parsing operator continuations.
/// Tracks both the operands AND the operator type used.
#[derive(Debug, Clone)]
enum ContinuationResult {
    /// No continuation found
    None,
    /// "or" operator with operands (union semantics)
    Or(Vec<Userset>),
    /// "and" operator with operands (intersection semantics)
    And(Vec<Userset>),
}

/// Parse additional or/and operands after a type constraint (respects precedence).
/// Returns the operator type along with the operands to ensure correct semantics.
fn parse_userset_continuation<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, ContinuationResult, E> {
    // Try parsing "or" operands first - each operand is at intersection level for proper precedence
    let (rest, or_ops) = many0(preceded(
        tuple((space0, tag("or"), space1)),
        parse_intersection_level,
    ))(input)?;

    if !or_ops.is_empty() {
        return Ok((rest, ContinuationResult::Or(or_ops)));
    }

    // Try parsing "and" operands
    let (rest, and_ops) = many0(preceded(
        tuple((space0, tag("and"), space1)),
        parse_exclusion_or_base,
    ))(input)?;

    if !and_ops.is_empty() {
        return Ok((rest, ContinuationResult::And(and_ops)));
    }

    // No continuations found
    Ok((input, ContinuationResult::None))
}

// ============ Relation Definition Parser ============

/// Parse a relation definition like "define viewer: [user] or editor"
fn parse_relation_definition<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, RelationDefinition, E> {
    context(
        "relation definition",
        map(
            tuple((
                space0,
                define_keyword,
                space1,
                identifier,
                char(':'),
                space0,
                opt(type_constraint),
                opt(preceded(space0, parse_userset)),
                // Also check for continuations after type constraint (e.g., "[user] or owner")
                parse_userset_continuation,
            )),
            |(_, _, _, name, _, _, type_constraint, userset, continuation): (
                _,
                _,
                _,
                &str,
                _,
                _,
                Option<Vec<String>>,
                Option<Userset>,
                ContinuationResult,
            )| {
                let base_userset = if let Some(explicit_userset) = userset {
                    // Explicit userset provided (e.g., "this" or "viewer from parent")
                    explicit_userset
                } else if type_constraint.is_some() {
                    // Type constraint implies This
                    Userset::This
                } else {
                    // No type constraint or userset, defaults to This
                    Userset::This
                };

                // Combine base userset with continuations using the correct operator
                let rewrite = match continuation {
                    ContinuationResult::None => base_userset,
                    ContinuationResult::Or(operands) => {
                        // [user] or owner -> Union(This, ComputedUserset(owner))
                        let mut children = vec![base_userset];
                        children.extend(operands);
                        Userset::Union { children }
                    }
                    ContinuationResult::And(operands) => {
                        // [user] and admin -> Intersection(This, ComputedUserset(admin))
                        let mut children = vec![base_userset];
                        children.extend(operands);
                        Userset::Intersection { children }
                    }
                };

                RelationDefinition {
                    name: name.to_string(),
                    type_constraints: type_constraint.unwrap_or_default(),
                    rewrite,
                }
            },
        ),
    )(input)
}

// ============ Type Definition Parser ============

/// Parse a type definition with optional relations
fn parse_type_definition<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, TypeDefinition, E> {
    context(
        "type definition",
        map(
            tuple((
                type_keyword,
                space1,
                identifier,
                ws,
                opt(preceded(
                    tuple((relations_keyword, ws)),
                    many0(terminated(parse_relation_definition, ws)),
                )),
            )),
            |(_, _, type_name, _, relations): (_, _, &str, _, _)| TypeDefinition {
                type_name: type_name.to_string(),
                relations: relations.unwrap_or_default(),
            },
        ),
    )(input)
}

// ============ Model Parser ============

/// Parse a complete authorization model
fn parse_model<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, AuthorizationModel, E> {
    context(
        "authorization model",
        map(
            tuple((ws, many0(terminated(parse_type_definition, ws)))),
            |(_, type_definitions)| AuthorizationModel {
                id: None,
                schema_version: "1.1".to_string(),
                type_definitions,
            },
        ),
    )(input)
}

// ============ Public API ============

/// Parse a DSL string into an AuthorizationModel.
///
/// # Example
///
/// ```ignore
/// let dsl = r#"
/// type user
///
/// type document
///   relations
///     define owner: [user]
///     define viewer: [user] or owner
/// "#;
///
/// let model = parse(dsl)?;
/// ```
pub fn parse(input: &str) -> ParserResult<AuthorizationModel> {
    match all_consuming(parse_model::<nom::error::VerboseError<&str>>)(input) {
        Ok((_, model)) => Ok(model),
        Err(nom::Err::Error(e)) | Err(nom::Err::Failure(e)) => Err(ParserError::new(format!(
            "Parse error: {}",
            nom::error::convert_error(input, e)
        ))),
        Err(nom::Err::Incomplete(_)) => Err(ParserError::new("Incomplete input")),
    }
}

/// Check if the input contains the "type" keyword.
#[cfg(test)]
fn has_type_keyword(input: &str) -> bool {
    input.contains("type")
}

/// Check if the input contains the "define" keyword.
#[cfg(test)]
fn has_define_keyword(input: &str) -> bool {
    input.contains("define")
}

/// Check if the input contains the "relations" keyword.
#[cfg(test)]
fn has_relations_keyword(input: &str) -> bool {
    input.contains("relations")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== Keyword Recognition Tests ==========

    #[test]
    fn test_parser_recognizes_type_keyword() {
        assert!(has_type_keyword("type user"));
        assert!(has_type_keyword("type document"));
        assert!(!has_type_keyword("define owner"));
    }

    #[test]
    fn test_parser_recognizes_define_keyword() {
        assert!(has_define_keyword("define owner: [user]"));
        assert!(!has_define_keyword("type user"));
    }

    #[test]
    fn test_parser_recognizes_relations_keyword() {
        assert!(has_relations_keyword("relations"));
        assert!(!has_relations_keyword("type user"));
    }

    // ========== Simple Type Definition Tests ==========

    #[test]
    fn test_parser_parses_simple_type_definition() {
        let input = "type user";
        let result = parse(input);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let model = result.unwrap();
        assert_eq!(model.type_definitions.len(), 1);
        assert_eq!(model.type_definitions[0].type_name, "user");
        assert!(model.type_definitions[0].relations.is_empty());
    }

    #[test]
    fn test_parser_parses_type_with_single_relation() {
        let input = r#"
type document
  relations
    define owner: [user]
"#;
        let result = parse(input);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let model = result.unwrap();
        assert_eq!(model.type_definitions.len(), 1);
        assert_eq!(model.type_definitions[0].type_name, "document");
        assert_eq!(model.type_definitions[0].relations.len(), 1);
        assert_eq!(model.type_definitions[0].relations[0].name, "owner");
    }

    #[test]
    fn test_parser_parses_type_with_multiple_relations() {
        let input = r#"
type document
  relations
    define owner: [user]
    define editor: [user]
    define viewer: [user]
"#;
        let result = parse(input);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let model = result.unwrap();
        assert_eq!(model.type_definitions[0].relations.len(), 3);
        assert_eq!(model.type_definitions[0].relations[0].name, "owner");
        assert_eq!(model.type_definitions[0].relations[1].name, "editor");
        assert_eq!(model.type_definitions[0].relations[2].name, "viewer");
    }

    // ========== Relation Type Tests ==========

    #[test]
    fn test_parser_parses_direct_relation_assignment() {
        let input = r#"
type document
  relations
    define owner: [user]
"#;
        let result = parse(input);
        assert!(result.is_ok());
        let model = result.unwrap();
        let relation = &model.type_definitions[0].relations[0];
        assert_eq!(relation.name, "owner");
        // Direct assignment with type constraint uses This
        assert!(matches!(relation.rewrite, Userset::This));
    }

    #[test]
    fn test_parser_parses_this_keyword() {
        let input = r#"
type document
  relations
    define owner: [user] this
"#;
        let result = parse(input);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let model = result.unwrap();
        let relation = &model.type_definitions[0].relations[0];
        assert!(matches!(relation.rewrite, Userset::This));
    }

    #[test]
    fn test_parser_parses_union_relation() {
        let input = r#"
type document
  relations
    define owner: [user]
    define viewer: [user] or owner
"#;
        let result = parse(input);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let model = result.unwrap();
        let viewer = &model.type_definitions[0].relations[1];
        assert_eq!(viewer.name, "viewer");
        match &viewer.rewrite {
            Userset::Union { children } => {
                assert_eq!(children.len(), 2);
            }
            _ => panic!("Expected Union, got {:?}", viewer.rewrite),
        }
    }

    #[test]
    fn test_parser_parses_intersection_relation() {
        let input = r#"
type document
  relations
    define owner: [user]
    define editor: [user]
    define admin: [user] owner and editor
"#;
        let result = parse(input);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let model = result.unwrap();
        let admin = &model.type_definitions[0].relations[2];
        assert_eq!(admin.name, "admin");
        match &admin.rewrite {
            Userset::Intersection { children } => {
                assert_eq!(children.len(), 2);
            }
            _ => panic!("Expected Intersection, got {:?}", admin.rewrite),
        }
    }

    #[test]
    fn test_parser_parses_exclusion_relation() {
        let input = r#"
type document
  relations
    define owner: [user]
    define blocked: [user]
    define viewer: [user] owner but not blocked
"#;
        let result = parse(input);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let model = result.unwrap();
        let viewer = &model.type_definitions[0].relations[2];
        assert_eq!(viewer.name, "viewer");
        match &viewer.rewrite {
            Userset::Exclusion { base, subtract } => {
                assert!(
                    matches!(base.as_ref(), Userset::ComputedUserset { relation } if relation == "owner")
                );
                assert!(
                    matches!(subtract.as_ref(), Userset::ComputedUserset { relation } if relation == "blocked")
                );
            }
            _ => panic!("Expected Exclusion, got {:?}", viewer.rewrite),
        }
    }

    #[test]
    fn test_parser_parses_computed_relation_from_parent() {
        let input = r#"
type folder
  relations
    define viewer: [user]

type document
  relations
    define parent: [folder]
    define viewer: [user] viewer from parent
"#;
        let result = parse(input);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let model = result.unwrap();
        assert_eq!(model.type_definitions.len(), 2);
        let doc_viewer = &model.type_definitions[1].relations[1];
        assert_eq!(doc_viewer.name, "viewer");
        match &doc_viewer.rewrite {
            Userset::TupleToUserset {
                tupleset,
                computed_userset,
            } => {
                assert_eq!(tupleset, "parent");
                assert_eq!(computed_userset, "viewer");
            }
            _ => panic!("Expected TupleToUserset, got {:?}", doc_viewer.rewrite),
        }
    }

    #[test]
    fn test_parser_handles_mixed_and_or_precedence() {
        // "and" should bind tighter than "or"
        // "editor and owner or reader" should parse as "(editor and owner) or reader"
        let input = r#"
type document
  relations
    define editor: [user]
    define owner: [user]
    define reader: [user]
    define access: editor and owner or reader
"#;
        let result = parse(input);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let model = result.unwrap();
        let access = &model.type_definitions[0].relations[3];
        assert_eq!(access.name, "access");

        // Should be Union { [Intersection { [editor, owner] }, reader] }
        match &access.rewrite {
            Userset::Union { children } => {
                assert_eq!(children.len(), 2, "Expected 2 children in union");
                // First child should be an intersection
                match &children[0] {
                    Userset::Intersection { children: inner } => {
                        assert_eq!(inner.len(), 2, "Expected 2 children in intersection");
                    }
                    _ => panic!(
                        "Expected first child to be Intersection, got {:?}",
                        children[0]
                    ),
                }
                // Second child should be computed userset (reader)
                match &children[1] {
                    Userset::ComputedUserset { relation } => {
                        assert_eq!(relation, "reader");
                    }
                    _ => panic!(
                        "Expected second child to be ComputedUserset, got {:?}",
                        children[1]
                    ),
                }
            }
            _ => panic!("Expected Union, got {:?}", access.rewrite),
        }
    }

    #[test]
    fn test_parser_type_constraint_with_and_produces_intersection() {
        // This is the critical test for issues #21/#22:
        // "[user] and admin" should produce Intersection(This, ComputedUserset(admin))
        // NOT Union(This, ComputedUserset(admin)) which was the bug
        let input = r#"
type user

type document
  relations
    define admin: [user]
    define restricted_viewer: [user] and admin
"#;
        let result = parse(input);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let model = result.unwrap();
        let restricted_viewer = &model.type_definitions[1].relations[1];
        assert_eq!(restricted_viewer.name, "restricted_viewer");

        // Must be Intersection, not Union!
        match &restricted_viewer.rewrite {
            Userset::Intersection { children } => {
                assert_eq!(children.len(), 2, "Expected 2 children in intersection");
                // First child should be This (from [user])
                match &children[0] {
                    Userset::This => {}
                    _ => panic!("Expected first child to be This, got {:?}", children[0]),
                }
                // Second child should be ComputedUserset(admin)
                match &children[1] {
                    Userset::ComputedUserset { relation } => {
                        assert_eq!(relation, "admin");
                    }
                    _ => panic!(
                        "Expected second child to be ComputedUserset, got {:?}",
                        children[1]
                    ),
                }
            }
            Userset::Union { .. } => {
                panic!(
                    "BUG: Got Union instead of Intersection! \
                     '[user] and admin' should be Intersection, not Union. \
                     Got: {:?}",
                    restricted_viewer.rewrite
                );
            }
            _ => panic!(
                "Expected Intersection, got {:?}",
                restricted_viewer.rewrite
            ),
        }
    }

    #[test]
    fn test_parser_type_constraint_with_or_produces_union() {
        // Verify [user] or admin produces Union (this was already working)
        let input = r#"
type user

type document
  relations
    define admin: [user]
    define viewer: [user] or admin
"#;
        let result = parse(input);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let model = result.unwrap();
        let viewer = &model.type_definitions[1].relations[1];
        assert_eq!(viewer.name, "viewer");

        match &viewer.rewrite {
            Userset::Union { children } => {
                assert_eq!(children.len(), 2, "Expected 2 children in union");
                match &children[0] {
                    Userset::This => {}
                    _ => panic!("Expected first child to be This, got {:?}", children[0]),
                }
                match &children[1] {
                    Userset::ComputedUserset { relation } => {
                        assert_eq!(relation, "admin");
                    }
                    _ => panic!(
                        "Expected second child to be ComputedUserset, got {:?}",
                        children[1]
                    ),
                }
            }
            _ => panic!("Expected Union, got {:?}", viewer.rewrite),
        }
    }

    // ========== Error Handling Tests ==========

    #[test]
    fn test_parser_rejects_invalid_syntax_with_clear_error() {
        let input = "invalid syntax here";
        let result = parse(input);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(!err.message.is_empty());
    }

    #[test]
    fn test_parser_rejects_incomplete_type_definition() {
        let input = "type"; // Missing type name
        let result = parse(input);
        assert!(result.is_err());
    }

    // ========== Whitespace Handling Tests ==========

    #[test]
    fn test_parser_handles_whitespace_correctly() {
        // Extra whitespace should be handled
        let input = r#"

type   user


type    document
  relations
    define    owner:   [user]

"#;
        let result = parse(input);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let model = result.unwrap();
        assert_eq!(model.type_definitions.len(), 2);
    }

    // ========== Comment Handling Tests ==========

    #[test]
    fn test_parser_handles_comments() {
        let input = r#"
# This is a comment
type user

# Another comment
type document
  relations
    # Relation comment
    define owner: [user]
"#;
        let result = parse(input);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let model = result.unwrap();
        assert_eq!(model.type_definitions.len(), 2);
    }

    // ========== Complete Model Tests ==========

    #[test]
    fn test_parser_parses_complete_model() {
        let input = r#"
type user

type document
  relations
    define owner: [user]
    define editor: [user] or owner
    define viewer: [user] or editor
"#;
        let result = parse(input);
        assert!(result.is_ok(), "Failed to parse: {:?}", result.err());
        let model = result.unwrap();

        assert_eq!(model.schema_version, "1.1");
        assert_eq!(model.type_definitions.len(), 2);

        // Verify user type
        assert_eq!(model.type_definitions[0].type_name, "user");
        assert!(model.type_definitions[0].relations.is_empty());

        // Verify document type
        assert_eq!(model.type_definitions[1].type_name, "document");
        assert_eq!(model.type_definitions[1].relations.len(), 3);
    }
}
