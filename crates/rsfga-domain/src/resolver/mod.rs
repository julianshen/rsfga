//! Graph resolver for permission checks.
//!
//! The resolver performs async graph traversal to determine
//! if a user has a specific permission on an object.
//!
//! # Architecture Decisions
//!
//! - **Parallel Execution (ADR-003)**: Union and intersection operations use
//!   `FuturesUnordered` for parallel branch evaluation with short-circuiting.
//!   Performance benefits are UNVALIDATED - benchmarks needed in M1.7.
//!
//! - **Cycle Detection**: Tracks visited nodes to prevent infinite loops.
//!   Uses `Arc<HashSet>` for efficient cloning during traversal.
//!   Memory growth is O(depth) per path. Profile in M1.7 if needed.
//!
//! - **Depth Limiting**: Default max depth of 25 matches OpenFGA behavior.
//!   Prevents stack overflow and DoS attacks (ADR-003, Constraint C11).
//!
//! - **Timeout Handling**: Configurable timeout (default 30s) prevents
//!   hanging on pathological graphs.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::time::timeout;

use crate::error::{DomainError, DomainResult};
use crate::model::{AuthorizationModel, RelationDefinition, TypeDefinition, Userset};

/// Configuration for the graph resolver.
#[derive(Debug, Clone)]
pub struct ResolverConfig {
    /// Maximum depth for graph traversal (matches OpenFGA default of 25).
    pub max_depth: u32,
    /// Timeout for check operations.
    pub timeout: Duration,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            max_depth: 25,
            timeout: Duration::from_secs(30),
        }
    }
}

/// Request for a permission check.
#[derive(Debug, Clone)]
pub struct CheckRequest {
    /// The store ID to check against.
    pub store_id: String,
    /// The user identifier (e.g., "user:alice").
    pub user: String,
    /// The relation to check (e.g., "viewer").
    pub relation: String,
    /// The object identifier (e.g., "document:readme").
    pub object: String,
    /// Contextual tuples to consider during the check.
    /// Wrapped in Arc for cheap cloning during graph traversal.
    pub contextual_tuples: Arc<Vec<ContextualTuple>>,
}

impl CheckRequest {
    /// Creates a new CheckRequest with contextual tuples.
    pub fn new(
        store_id: String,
        user: String,
        relation: String,
        object: String,
        contextual_tuples: Vec<ContextualTuple>,
    ) -> Self {
        Self {
            store_id,
            user,
            relation,
            object,
            contextual_tuples: Arc::new(contextual_tuples),
        }
    }
}

/// A contextual tuple for temporary authorization during a check.
#[derive(Debug, Clone)]
pub struct ContextualTuple {
    pub user: String,
    pub relation: String,
    pub object: String,
}

/// Result of a permission check.
#[derive(Debug, Clone)]
pub struct CheckResult {
    /// Whether the check is allowed.
    pub allowed: bool,
}

/// Internal context for graph traversal.
#[derive(Debug, Clone)]
struct TraversalContext {
    /// Current traversal depth.
    depth: u32,
    /// Visited nodes for cycle detection (object:relation pairs).
    /// Wrapped in Arc for cheap cloning when not mutating.
    visited: Arc<HashSet<String>>,
}

impl TraversalContext {
    fn new() -> Self {
        Self {
            depth: 0,
            visited: Arc::new(HashSet::new()),
        }
    }

    fn increment_depth(&self) -> Self {
        Self {
            depth: self.depth + 1,
            visited: Arc::clone(&self.visited),
        }
    }

    fn with_visited(&self, key: &str) -> Self {
        // Clone the inner HashSet only when adding new entries (copy-on-write)
        let mut new_visited = (*self.visited).clone();
        new_visited.insert(key.to_string());
        Self {
            depth: self.depth,
            visited: Arc::new(new_visited),
        }
    }
}

/// Trait for tuple storage operations needed by the resolver.
#[async_trait]
pub trait TupleReader: Send + Sync {
    /// Reads tuples matching the given criteria.
    async fn read_tuples(
        &self,
        store_id: &str,
        object_type: &str,
        object_id: &str,
        relation: &str,
    ) -> DomainResult<Vec<StoredTupleRef>>;

    /// Checks if a store exists.
    async fn store_exists(&self, store_id: &str) -> DomainResult<bool>;
}

/// Reference to a stored tuple for resolver use.
#[derive(Debug, Clone)]
pub struct StoredTupleRef {
    pub user_type: String,
    pub user_id: String,
    pub user_relation: Option<String>,
}

/// Trait for authorization model operations needed by the resolver.
#[async_trait]
pub trait ModelReader: Send + Sync {
    /// Gets the authorization model for a store.
    async fn get_model(&self, store_id: &str) -> DomainResult<AuthorizationModel>;

    /// Gets a type definition by name.
    async fn get_type_definition(
        &self,
        store_id: &str,
        type_name: &str,
    ) -> DomainResult<TypeDefinition>;

    /// Gets a relation definition from a type.
    async fn get_relation_definition(
        &self,
        store_id: &str,
        type_name: &str,
        relation: &str,
    ) -> DomainResult<RelationDefinition>;
}

/// Graph resolver for permission checks.
///
/// Performs async graph traversal to determine if a user has
/// a specific permission on an object.
pub struct GraphResolver<T, M> {
    tuple_reader: Arc<T>,
    model_reader: Arc<M>,
    config: ResolverConfig,
}

/// Type alias for boxed future to handle async recursion.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

impl<T, M> GraphResolver<T, M>
where
    T: TupleReader + 'static,
    M: ModelReader + 'static,
{
    /// Creates a new graph resolver.
    pub fn new(tuple_reader: Arc<T>, model_reader: Arc<M>) -> Self {
        Self {
            tuple_reader,
            model_reader,
            config: ResolverConfig::default(),
        }
    }

    /// Creates a new graph resolver with custom configuration.
    pub fn with_config(tuple_reader: Arc<T>, model_reader: Arc<M>, config: ResolverConfig) -> Self {
        Self {
            tuple_reader,
            model_reader,
            config,
        }
    }

    /// Performs a permission check.
    pub async fn check(&self, request: &CheckRequest) -> DomainResult<CheckResult> {
        // Validate inputs
        self.validate_request(request)?;

        // Check if store exists
        if !self.tuple_reader.store_exists(&request.store_id).await? {
            return Err(DomainError::ResolverError {
                message: format!("store not found: {}", request.store_id),
            });
        }

        // Apply timeout to the entire check operation
        let ctx = TraversalContext::new();
        let check_future = self.resolve_check(request.clone(), ctx);

        match timeout(self.config.timeout, check_future).await {
            Ok(result) => result,
            Err(_) => Err(DomainError::Timeout {
                duration_ms: self.config.timeout.as_millis() as u64,
            }),
        }
    }

    /// Validates the check request.
    fn validate_request(&self, request: &CheckRequest) -> DomainResult<()> {
        // Validate user format
        if request.user.is_empty() {
            return Err(DomainError::InvalidUserFormat {
                value: request.user.clone(),
            });
        }

        // User must be in type:id format (unless wildcard)
        if request.user != "*" {
            if !request.user.contains(':') {
                return Err(DomainError::InvalidUserFormat {
                    value: request.user.clone(),
                });
            }
            // Validate user parts (both type and id must be non-empty)
            let parts: Vec<&str> = request.user.splitn(2, ':').collect();
            if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
                return Err(DomainError::InvalidUserFormat {
                    value: request.user.clone(),
                });
            }
        }

        // Validate object format (must be type:id)
        if request.object.is_empty() {
            return Err(DomainError::InvalidObjectFormat {
                value: request.object.clone(),
            });
        }

        if !request.object.contains(':') {
            return Err(DomainError::InvalidObjectFormat {
                value: request.object.clone(),
            });
        }

        // Validate object parts
        let parts: Vec<&str> = request.object.splitn(2, ':').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(DomainError::InvalidObjectFormat {
                value: request.object.clone(),
            });
        }

        // Validate relation
        if request.relation.is_empty() {
            return Err(DomainError::InvalidRelationFormat {
                value: request.relation.clone(),
            });
        }

        Ok(())
    }

    /// Internal check resolution with traversal context (boxed for recursion).
    fn resolve_check(
        &self,
        request: CheckRequest,
        ctx: TraversalContext,
    ) -> BoxFuture<'_, DomainResult<CheckResult>> {
        Box::pin(async move {
            // Check depth limit
            if ctx.depth >= self.config.max_depth {
                return Err(DomainError::DepthLimitExceeded {
                    max_depth: self.config.max_depth,
                });
            }

            // Create cycle detection key
            let cycle_key = format!(
                "{}:{}#{}",
                request.store_id, request.object, request.relation
            );

            // Check for cycles
            if ctx.visited.contains(&cycle_key) {
                return Err(DomainError::CycleDetected { path: cycle_key });
            }

            // Parse object to get type and id (convert to owned immediately)
            let (object_type, object_id) = self.parse_object(&request.object)?;
            let object_type = object_type.to_string();
            let object_id = object_id.to_string();

            // Get the relation definition
            let relation_def = self
                .model_reader
                .get_relation_definition(&request.store_id, &object_type, &request.relation)
                .await?;

            // Add current node to visited set
            let ctx = ctx.with_visited(&cycle_key);

            // Resolve based on the userset rewrite
            self.resolve_userset(request, relation_def.rewrite, object_type, object_id, ctx)
                .await
        })
    }

    /// Resolves a userset rewrite (boxed for recursion).
    fn resolve_userset(
        &self,
        request: CheckRequest,
        userset: Userset,
        object_type: String,
        object_id: String,
        ctx: TraversalContext,
    ) -> BoxFuture<'_, DomainResult<CheckResult>> {
        Box::pin(async move {
            match userset {
                Userset::This => {
                    // Direct tuple lookup
                    self.resolve_direct(request, object_type, object_id, ctx)
                        .await
                }

                Userset::ComputedUserset { relation } => {
                    // Check another relation on the same object
                    let new_request = CheckRequest {
                        store_id: request.store_id,
                        user: request.user,
                        relation,
                        object: request.object,
                        contextual_tuples: request.contextual_tuples,
                    };
                    let new_ctx = ctx.increment_depth();
                    self.resolve_check(new_request, new_ctx).await
                }

                Userset::TupleToUserset {
                    tupleset,
                    computed_userset,
                } => {
                    // Resolve relation from parent object
                    self.resolve_tuple_to_userset(
                        request,
                        &tupleset,
                        &computed_userset,
                        &object_type,
                        &object_id,
                        ctx,
                    )
                    .await
                }

                Userset::Union { children } => {
                    // Any child must be true (parallel execution with short-circuit)
                    self.resolve_union(request, children, object_type, object_id, ctx)
                        .await
                }

                Userset::Intersection { children } => {
                    // All children must be true (parallel execution with short-circuit)
                    self.resolve_intersection(request, children, object_type, object_id, ctx)
                        .await
                }

                Userset::Exclusion { base, subtract } => {
                    // Base must be true AND subtract must be false
                    self.resolve_exclusion(request, *base, *subtract, object_type, object_id, ctx)
                        .await
                }
            }
        })
    }

    /// Resolves a direct tuple assignment.
    fn resolve_direct(
        &self,
        request: CheckRequest,
        object_type: String,
        object_id: String,
        ctx: TraversalContext,
    ) -> BoxFuture<'_, DomainResult<CheckResult>> {
        Box::pin(async move {
            // First check contextual tuples
            for ct in request.contextual_tuples.iter() {
                if ct.object == request.object && ct.relation == request.relation {
                    // Check for direct match or type wildcard
                    if self.user_matches(&request.user, &ct.user) {
                        return Ok(CheckResult { allowed: true });
                    }

                    // Check for userset reference in contextual tuple (e.g., "team:eng#member")
                    if let Some((user_obj, user_rel)) = ct.user.split_once('#') {
                        // This is a userset reference, recursively resolve it
                        let userset_request = CheckRequest {
                            store_id: request.store_id.clone(),
                            user: request.user.clone(),
                            relation: user_rel.to_string(),
                            object: user_obj.to_string(),
                            contextual_tuples: request.contextual_tuples.clone(),
                        };

                        let new_ctx = ctx.increment_depth();
                        let result = self.resolve_check(userset_request, new_ctx.clone()).await?;
                        if result.allowed {
                            return Ok(CheckResult { allowed: true });
                        }
                    }
                }
            }

            // Read tuples from storage
            let tuples = self
                .tuple_reader
                .read_tuples(
                    &request.store_id,
                    &object_type,
                    &object_id,
                    &request.relation,
                )
                .await?;

            for tuple in tuples {
                let tuple_user = if let Some(ref rel) = tuple.user_relation {
                    format!("{}:{}#{}", tuple.user_type, tuple.user_id, rel)
                } else {
                    format!("{}:{}", tuple.user_type, tuple.user_id)
                };

                // Check for direct match or wildcard match (handled by user_matches)
                if self.user_matches(&request.user, &tuple_user) {
                    return Ok(CheckResult { allowed: true });
                }

                // Check for userset reference (e.g., group:eng#member)
                if let Some(userset_relation) = &tuple.user_relation {
                    let userset_object = format!("{}:{}", tuple.user_type, tuple.user_id);

                    let userset_request = CheckRequest {
                        store_id: request.store_id.clone(),
                        user: request.user.clone(),
                        relation: userset_relation.clone(),
                        object: userset_object,
                        contextual_tuples: request.contextual_tuples.clone(),
                    };

                    let new_ctx = ctx.increment_depth();
                    let result = self.resolve_check(userset_request, new_ctx).await?;
                    if result.allowed {
                        return Ok(CheckResult { allowed: true });
                    }
                }
            }

            Ok(CheckResult { allowed: false })
        })
    }

    /// Resolves a tuple-to-userset relation (e.g., viewer from parent).
    async fn resolve_tuple_to_userset(
        &self,
        request: CheckRequest,
        tupleset: &str,
        computed_userset: &str,
        object_type: &str,
        object_id: &str,
        ctx: TraversalContext,
    ) -> DomainResult<CheckResult> {
        // Read tuples for the tupleset relation
        let tuples = self
            .tuple_reader
            .read_tuples(&request.store_id, object_type, object_id, tupleset)
            .await?;

        for tuple in tuples {
            // The parent object is the user of the tupleset tuple
            let parent_object = format!("{}:{}", tuple.user_type, tuple.user_id);

            // Check if user has the computed_userset relation on the parent
            let parent_request = CheckRequest {
                store_id: request.store_id.clone(),
                user: request.user.clone(),
                relation: computed_userset.to_string(),
                object: parent_object,
                contextual_tuples: request.contextual_tuples.clone(),
            };

            let new_ctx = ctx.increment_depth();
            let result = self.resolve_check(parent_request, new_ctx).await?;
            if result.allowed {
                return Ok(CheckResult { allowed: true });
            }
        }

        Ok(CheckResult { allowed: false })
    }

    /// Resolves a union of usersets (any child must be true).
    ///
    /// Uses `FuturesUnordered` for parallel branch evaluation with short-circuiting
    /// on first success (ADR-003). Performance benefits are unvalidated - see M1.7.
    async fn resolve_union(
        &self,
        request: CheckRequest,
        children: Vec<Userset>,
        object_type: String,
        object_id: String,
        ctx: TraversalContext,
    ) -> DomainResult<CheckResult> {
        let new_ctx = ctx.increment_depth();

        // Create FuturesUnordered for parallel execution with short-circuiting
        let mut futures: FuturesUnordered<_> = children
            .into_iter()
            .map(|child| {
                self.resolve_userset(
                    request.clone(),
                    child,
                    object_type.clone(),
                    object_id.clone(),
                    new_ctx.clone(),
                )
            })
            .collect();

        // Track errors for reporting if all branches fail
        let mut last_error: Option<DomainError> = None;
        let mut cycle_error: Option<DomainError> = None;
        let mut had_false_result = false;

        // Poll futures and short-circuit on first allowed=true
        while let Some(result) = futures.next().await {
            match result {
                Ok(CheckResult { allowed: true }) => {
                    // Short-circuit: found a branch that allows access
                    return Ok(CheckResult { allowed: true });
                }
                Ok(CheckResult { allowed: false }) => {
                    // Continue checking other branches
                    had_false_result = true;
                }
                Err(e) => {
                    // Track cycle errors separately - they're only fatal if ALL branches cycle
                    if matches!(e, DomainError::CycleDetected { .. }) {
                        cycle_error = Some(e);
                    } else {
                        last_error = Some(e);
                    }
                }
            }
        }

        // If we had a non-cycle error and no branch succeeded, propagate it
        if let Some(e) = last_error {
            return Err(e);
        }

        // If ALL branches returned cycle errors (no false results), propagate the cycle
        if !had_false_result {
            if let Some(e) = cycle_error {
                return Err(e);
            }
        }

        Ok(CheckResult { allowed: false })
    }

    /// Resolves an intersection of usersets (all children must be true).
    ///
    /// Uses `FuturesUnordered` for parallel branch evaluation with short-circuiting
    /// on first failure (ADR-003). Performance benefits are unvalidated - see M1.7.
    async fn resolve_intersection(
        &self,
        request: CheckRequest,
        children: Vec<Userset>,
        object_type: String,
        object_id: String,
        ctx: TraversalContext,
    ) -> DomainResult<CheckResult> {
        let new_ctx = ctx.increment_depth();

        // Create FuturesUnordered for parallel execution with short-circuiting
        let mut futures: FuturesUnordered<_> = children
            .into_iter()
            .map(|child| {
                self.resolve_userset(
                    request.clone(),
                    child,
                    object_type.clone(),
                    object_id.clone(),
                    new_ctx.clone(),
                )
            })
            .collect();

        // Poll futures and short-circuit on first allowed=false or error
        while let Some(result) = futures.next().await {
            match result {
                Ok(CheckResult { allowed: true }) => {
                    // Continue checking other branches
                }
                Ok(CheckResult { allowed: false }) => {
                    // Short-circuit: found a branch that denies access
                    return Ok(CheckResult { allowed: false });
                }
                Err(e) => {
                    // Short-circuit on error
                    return Err(e);
                }
            }
        }

        // All branches returned allowed=true
        Ok(CheckResult { allowed: true })
    }

    /// Resolves an exclusion (base must be true AND subtract must be false).
    ///
    /// Error handling (ADR-003):
    /// - If base is false → return false (don't need subtract)
    /// - If subtract is true → return false (don't need base)
    /// - Cycle errors only propagate when the errored branch's result is needed
    async fn resolve_exclusion(
        &self,
        request: CheckRequest,
        base: Userset,
        subtract: Userset,
        object_type: String,
        object_id: String,
        ctx: TraversalContext,
    ) -> DomainResult<CheckResult> {
        let new_ctx = ctx.increment_depth();

        // Execute both in parallel
        let (base_result, subtract_result) = futures::future::join(
            self.resolve_userset(
                request.clone(),
                base,
                object_type.clone(),
                object_id.clone(),
                new_ctx.clone(),
            ),
            self.resolve_userset(request, subtract, object_type, object_id, new_ctx),
        )
        .await;

        // Handle results with proper error semantics
        // Exclusion: base AND NOT subtract
        match (base_result, subtract_result) {
            // Both succeeded - normal case
            (Ok(base), Ok(subtract)) => Ok(CheckResult {
                allowed: base.allowed && !subtract.allowed,
            }),

            // Base is false - result is false regardless of subtract
            (Ok(CheckResult { allowed: false }), _) => Ok(CheckResult { allowed: false }),

            // Subtract is true - result is false regardless of base
            (_, Ok(CheckResult { allowed: true })) => Ok(CheckResult { allowed: false }),

            // Base is true, subtract errored - we need subtract, propagate error
            (Ok(CheckResult { allowed: true }), Err(e)) => Err(e),

            // Base errored, subtract is false - we need base, propagate error
            (Err(e), Ok(CheckResult { allowed: false })) => Err(e),

            // Both errored - propagate cycle error if present, otherwise base error
            (Err(base_err), Err(subtract_err)) => {
                // Prefer cycle errors as they're more specific
                if matches!(base_err, DomainError::CycleDetected { .. }) {
                    Err(base_err)
                } else if matches!(subtract_err, DomainError::CycleDetected { .. }) {
                    Err(subtract_err)
                } else {
                    Err(base_err)
                }
            }
        }
    }

    /// Checks if a user matches a target user string.
    fn user_matches(&self, requesting_user: &str, tuple_user: &str) -> bool {
        if requesting_user == tuple_user {
            return true;
        }

        // Check for type wildcard (e.g., user:* matches user:alice)
        if tuple_user.ends_with(":*") {
            if let Some((tuple_type, _)) = tuple_user.split_once(':') {
                if let Some((user_type, _)) = requesting_user.split_once(':') {
                    return tuple_type == user_type;
                }
            }
        }

        false
    }

    /// Parses an object string into type and id.
    fn parse_object<'a>(&self, object: &'a str) -> DomainResult<(&'a str, &'a str)> {
        let parts: Vec<&str> = object.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(DomainError::InvalidObjectFormat {
                value: object.to_string(),
            });
        }
        Ok((parts[0], parts[1]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tokio::sync::RwLock;

    // ========== Mock Implementations ==========

    /// Mock tuple reader for testing.
    struct MockTupleReader {
        stores: RwLock<HashSet<String>>,
        tuples: RwLock<HashMap<String, Vec<StoredTupleRef>>>,
    }

    impl MockTupleReader {
        fn new() -> Self {
            Self {
                stores: RwLock::new(HashSet::new()),
                tuples: RwLock::new(HashMap::new()),
            }
        }

        async fn add_store(&self, store_id: &str) {
            self.stores.write().await.insert(store_id.to_string());
        }

        #[allow(clippy::too_many_arguments)]
        async fn add_tuple(
            &self,
            store_id: &str,
            object_type: &str,
            object_id: &str,
            relation: &str,
            user_type: &str,
            user_id: &str,
            user_relation: Option<&str>,
        ) {
            let key = format!("{}:{}:{}:{}", store_id, object_type, object_id, relation);
            let tuple = StoredTupleRef {
                user_type: user_type.to_string(),
                user_id: user_id.to_string(),
                user_relation: user_relation.map(|s| s.to_string()),
            };
            self.tuples
                .write()
                .await
                .entry(key)
                .or_default()
                .push(tuple);
        }

        async fn remove_tuple(
            &self,
            store_id: &str,
            object_type: &str,
            object_id: &str,
            relation: &str,
            user_type: &str,
            user_id: &str,
        ) {
            let key = format!("{}:{}:{}:{}", store_id, object_type, object_id, relation);
            if let Some(tuples) = self.tuples.write().await.get_mut(&key) {
                tuples.retain(|t| t.user_type != user_type || t.user_id != user_id);
            }
        }
    }

    #[async_trait]
    impl TupleReader for MockTupleReader {
        async fn read_tuples(
            &self,
            store_id: &str,
            object_type: &str,
            object_id: &str,
            relation: &str,
        ) -> DomainResult<Vec<StoredTupleRef>> {
            let key = format!("{}:{}:{}:{}", store_id, object_type, object_id, relation);
            Ok(self
                .tuples
                .read()
                .await
                .get(&key)
                .cloned()
                .unwrap_or_default())
        }

        async fn store_exists(&self, store_id: &str) -> DomainResult<bool> {
            Ok(self.stores.read().await.contains(store_id))
        }
    }

    /// Mock model reader for testing.
    struct MockModelReader {
        type_definitions: RwLock<HashMap<String, TypeDefinition>>,
    }

    impl MockModelReader {
        fn new() -> Self {
            Self {
                type_definitions: RwLock::new(HashMap::new()),
            }
        }

        async fn add_type(&self, store_id: &str, type_def: TypeDefinition) {
            let key = format!("{}:{}", store_id, type_def.type_name);
            self.type_definitions.write().await.insert(key, type_def);
        }
    }

    #[async_trait]
    impl ModelReader for MockModelReader {
        async fn get_model(&self, _store_id: &str) -> DomainResult<AuthorizationModel> {
            Ok(AuthorizationModel::new("1.1"))
        }

        async fn get_type_definition(
            &self,
            store_id: &str,
            type_name: &str,
        ) -> DomainResult<TypeDefinition> {
            let key = format!("{}:{}", store_id, type_name);
            self.type_definitions
                .read()
                .await
                .get(&key)
                .cloned()
                .ok_or_else(|| DomainError::TypeNotFound {
                    type_name: type_name.to_string(),
                })
        }

        async fn get_relation_definition(
            &self,
            store_id: &str,
            type_name: &str,
            relation: &str,
        ) -> DomainResult<RelationDefinition> {
            let type_def = self.get_type_definition(store_id, type_name).await?;
            type_def
                .relations
                .into_iter()
                .find(|r| r.name == relation)
                .ok_or_else(|| DomainError::RelationNotFound {
                    type_name: type_name.to_string(),
                    relation: relation.to_string(),
                })
        }
    }

    // ========== Section 1: Direct Tuple Resolution ==========

    #[tokio::test]
    async fn test_check_returns_true_for_direct_tuple_assignment() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        // Set up store
        tuple_reader.add_store("store1").await;

        // Add type definition with direct relation
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".to_string()],
                        rewrite: Userset::This,
                    }],
                },
            )
            .await;

        // Add tuple: user:alice is viewer of document:readme
        tuple_reader
            .add_tuple(
                "store1", "document", "readme", "viewer", "user", "alice", None,
            )
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(
            result.allowed,
            "Direct tuple assignment should allow access"
        );
    }

    #[tokio::test]
    async fn test_check_returns_false_when_no_tuple_exists() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".to_string()],
                        rewrite: Userset::This,
                    }],
                },
            )
            .await;

        // No tuple added

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(!result.allowed, "Should deny access when no tuple exists");
    }

    #[tokio::test]
    async fn test_check_handles_multiple_stores_independently() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        // Set up two stores
        tuple_reader.add_store("store1").await;
        tuple_reader.add_store("store2").await;

        // Add type definitions for both stores
        for store_id in &["store1", "store2"] {
            model_reader
                .add_type(
                    store_id,
                    TypeDefinition {
                        type_name: "document".to_string(),
                        relations: vec![RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        }],
                    },
                )
                .await;
        }

        // Add tuple only in store1
        tuple_reader
            .add_tuple(
                "store1", "document", "readme", "viewer", "user", "alice", None,
            )
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        // Check store1 - should be allowed
        let request1 = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };
        let result1 = resolver.check(&request1).await.unwrap();
        assert!(result1.allowed, "Store1 should allow access");

        // Check store2 - should be denied
        let request2 = CheckRequest {
            store_id: "store2".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };
        let result2 = resolver.check(&request2).await.unwrap();
        assert!(!result2.allowed, "Store2 should deny access (no tuple)");
    }

    #[tokio::test]
    async fn test_check_validates_input_parameters() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        // Empty user
        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };
        let result = resolver.check(&request).await;
        assert!(matches!(result, Err(DomainError::InvalidUserFormat { .. })));

        // Empty relation
        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "".to_string(),
            object: "document:readme".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };
        let result = resolver.check(&request).await;
        assert!(matches!(
            result,
            Err(DomainError::InvalidRelationFormat { .. })
        ));

        // Empty object
        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };
        let result = resolver.check(&request).await;
        assert!(matches!(
            result,
            Err(DomainError::InvalidObjectFormat { .. })
        ));
    }

    #[tokio::test]
    async fn test_check_rejects_invalid_user_format() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        // Invalid user format (no colon)
        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };
        let result = resolver.check(&request).await;
        assert!(matches!(result, Err(DomainError::InvalidUserFormat { .. })));
    }

    #[tokio::test]
    async fn test_check_rejects_invalid_object_format() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        // Invalid object format (no colon)
        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "readme".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };
        let result = resolver.check(&request).await;
        assert!(matches!(
            result,
            Err(DomainError::InvalidObjectFormat { .. })
        ));

        // Invalid object format (empty type)
        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: ":readme".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };
        let result = resolver.check(&request).await;
        assert!(matches!(
            result,
            Err(DomainError::InvalidObjectFormat { .. })
        ));

        // Invalid object format (empty id)
        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };
        let result = resolver.check(&request).await;
        assert!(matches!(
            result,
            Err(DomainError::InvalidObjectFormat { .. })
        ));
    }

    // ========== Section 2: Computed Relations ==========

    #[tokio::test]
    async fn test_can_resolve_this_relation() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // Relation with "this" rewrite
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![RelationDefinition {
                        name: "owner".to_string(),
                        type_constraints: vec!["user".to_string()],
                        rewrite: Userset::This,
                    }],
                },
            )
            .await;

        tuple_reader
            .add_tuple(
                "store1", "document", "readme", "owner", "user", "alice", None,
            )
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "owner".to_string(),
            object: "document:readme".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(result.allowed);
    }

    #[tokio::test]
    async fn test_can_resolve_relation_from_parent_object() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // folder type with viewer relation
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "folder".to_string(),
                    relations: vec![RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".to_string()],
                        rewrite: Userset::This,
                    }],
                },
            )
            .await;

        // document type: viewer = viewer from parent folder
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "parent".to_string(),
                            type_constraints: vec!["folder".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::TupleToUserset {
                                tupleset: "parent".to_string(),
                                computed_userset: "viewer".to_string(),
                            },
                        },
                    ],
                },
            )
            .await;

        // Set up: folder1 -> document1, alice is viewer of folder1
        tuple_reader
            .add_tuple(
                "store1", "folder", "folder1", "viewer", "user", "alice", None,
            )
            .await;
        tuple_reader
            .add_tuple(
                "store1", "document", "doc1", "parent", "folder", "folder1", None,
            )
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(
            result.allowed,
            "Should inherit viewer permission from parent folder"
        );
    }

    #[tokio::test]
    async fn test_resolves_nested_parent_relationships() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // org type with admin relation
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "org".to_string(),
                    relations: vec![RelationDefinition {
                        name: "admin".to_string(),
                        type_constraints: vec!["user".to_string()],
                        rewrite: Userset::This,
                    }],
                },
            )
            .await;

        // folder type: admin = admin from parent org
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "folder".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "parent".to_string(),
                            type_constraints: vec!["org".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "admin".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::TupleToUserset {
                                tupleset: "parent".to_string(),
                                computed_userset: "admin".to_string(),
                            },
                        },
                    ],
                },
            )
            .await;

        // document type: admin = admin from parent folder
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "parent".to_string(),
                            type_constraints: vec!["folder".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "admin".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::TupleToUserset {
                                tupleset: "parent".to_string(),
                                computed_userset: "admin".to_string(),
                            },
                        },
                    ],
                },
            )
            .await;

        // org1 -> folder1 -> doc1, alice is admin of org1
        tuple_reader
            .add_tuple("store1", "org", "org1", "admin", "user", "alice", None)
            .await;
        tuple_reader
            .add_tuple("store1", "folder", "folder1", "parent", "org", "org1", None)
            .await;
        tuple_reader
            .add_tuple(
                "store1", "document", "doc1", "parent", "folder", "folder1", None,
            )
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "admin".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(
            result.allowed,
            "Should inherit admin permission through nested parents"
        );
    }

    #[tokio::test]
    async fn test_handles_missing_parent_gracefully() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "folder".to_string(),
                    relations: vec![RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".to_string()],
                        rewrite: Userset::This,
                    }],
                },
            )
            .await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "parent".to_string(),
                            type_constraints: vec!["folder".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::TupleToUserset {
                                tupleset: "parent".to_string(),
                                computed_userset: "viewer".to_string(),
                            },
                        },
                    ],
                },
            )
            .await;

        // No parent tuple exists for doc1

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(!result.allowed, "Should deny access when parent is missing");
    }

    // ========== Section 3: Union Relations (A or B) ==========

    #[tokio::test]
    async fn test_union_returns_true_if_any_branch_is_true() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // viewer = owner or direct_viewer
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "owner".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "direct_viewer".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::Union {
                                children: vec![
                                    Userset::ComputedUserset {
                                        relation: "owner".to_string(),
                                    },
                                    Userset::ComputedUserset {
                                        relation: "direct_viewer".to_string(),
                                    },
                                ],
                            },
                        },
                    ],
                },
            )
            .await;

        // Alice is owner (but not direct_viewer)
        tuple_reader
            .add_tuple("store1", "document", "doc1", "owner", "user", "alice", None)
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(
            result.allowed,
            "Union should return true if any branch is true"
        );
    }

    #[tokio::test]
    async fn test_union_returns_false_if_all_branches_are_false() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "owner".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "editor".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::Union {
                                children: vec![
                                    Userset::ComputedUserset {
                                        relation: "owner".to_string(),
                                    },
                                    Userset::ComputedUserset {
                                        relation: "editor".to_string(),
                                    },
                                ],
                            },
                        },
                    ],
                },
            )
            .await;

        // Alice has no permissions

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(
            !result.allowed,
            "Union should return false if all branches are false"
        );
    }

    #[tokio::test]
    async fn test_union_executes_branches_in_parallel() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // Create union with many branches
        let mut children = vec![];
        for i in 0..10 {
            children.push(Userset::ComputedUserset {
                relation: format!("branch{}", i),
            });
        }

        let mut relations = vec![];
        for i in 0..10 {
            relations.push(RelationDefinition {
                name: format!("branch{}", i),
                type_constraints: vec!["user".to_string()],
                rewrite: Userset::This,
            });
        }
        relations.push(RelationDefinition {
            name: "combined".to_string(),
            type_constraints: vec![],
            rewrite: Userset::Union { children },
        });

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations,
                },
            )
            .await;

        // Add tuple for branch5 only
        tuple_reader
            .add_tuple(
                "store1", "document", "doc1", "branch5", "user", "alice", None,
            )
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "combined".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(result.allowed, "Union with many branches should work");
    }

    #[tokio::test]
    async fn test_union_handles_errors_in_branches() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // Union with one valid branch and one that would have errors
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "owner".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::Union {
                                children: vec![
                                    Userset::ComputedUserset {
                                        relation: "owner".to_string(),
                                    },
                                    // This would cause an error if checked (relation doesn't exist)
                                    Userset::ComputedUserset {
                                        relation: "nonexistent".to_string(),
                                    },
                                ],
                            },
                        },
                    ],
                },
            )
            .await;

        // Alice is owner - this should return true despite the error in another branch
        tuple_reader
            .add_tuple("store1", "document", "doc1", "owner", "user", "alice", None)
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        // Should succeed because owner branch is true
        let result = resolver.check(&request).await.unwrap();
        assert!(result.allowed);
    }

    #[tokio::test]
    async fn test_union_returns_cycle_error_when_all_branches_cycle() {
        // Test that when ALL branches in a union return CycleDetected,
        // the error is properly propagated instead of returning allowed: false
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // Create a model where union branches both lead to cycles
        // branch1: viewer -> computed from cyclic_rel1
        // branch2: viewer -> computed from cyclic_rel2
        // cyclic_rel1 and cyclic_rel2 both reference viewer (creating cycles)
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::Union {
                                children: vec![
                                    Userset::ComputedUserset {
                                        relation: "cyclic_rel1".to_string(),
                                    },
                                    Userset::ComputedUserset {
                                        relation: "cyclic_rel2".to_string(),
                                    },
                                ],
                            },
                        },
                        RelationDefinition {
                            name: "cyclic_rel1".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::ComputedUserset {
                                relation: "viewer".to_string(),
                            },
                        },
                        RelationDefinition {
                            name: "cyclic_rel2".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::ComputedUserset {
                                relation: "viewer".to_string(),
                            },
                        },
                    ],
                },
            )
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        // Should return CycleDetected error, not allowed: false
        let result = resolver.check(&request).await;
        assert!(
            matches!(result, Err(DomainError::CycleDetected { .. })),
            "Union with all cyclic branches should return CycleDetected error, got {:?}",
            result
        );
    }

    // ========== Section 4: Intersection Relations (A and B) ==========

    #[tokio::test]
    async fn test_intersection_returns_true_only_if_all_branches_are_true() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // can_edit = is_member and is_verified
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "member".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "verified".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "can_edit".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::Intersection {
                                children: vec![
                                    Userset::ComputedUserset {
                                        relation: "member".to_string(),
                                    },
                                    Userset::ComputedUserset {
                                        relation: "verified".to_string(),
                                    },
                                ],
                            },
                        },
                    ],
                },
            )
            .await;

        // Alice is both member AND verified
        tuple_reader
            .add_tuple(
                "store1", "document", "doc1", "member", "user", "alice", None,
            )
            .await;
        tuple_reader
            .add_tuple(
                "store1", "document", "doc1", "verified", "user", "alice", None,
            )
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "can_edit".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(
            result.allowed,
            "Intersection should return true when all branches are true"
        );
    }

    #[tokio::test]
    async fn test_intersection_returns_false_if_any_branch_is_false() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "member".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "verified".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "can_edit".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::Intersection {
                                children: vec![
                                    Userset::ComputedUserset {
                                        relation: "member".to_string(),
                                    },
                                    Userset::ComputedUserset {
                                        relation: "verified".to_string(),
                                    },
                                ],
                            },
                        },
                    ],
                },
            )
            .await;

        // Alice is member but NOT verified
        tuple_reader
            .add_tuple(
                "store1", "document", "doc1", "member", "user", "alice", None,
            )
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "can_edit".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(
            !result.allowed,
            "Intersection should return false if any branch is false"
        );
    }

    #[tokio::test]
    async fn test_intersection_executes_branches_in_parallel() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // Create intersection with many branches
        let mut children = vec![];
        for i in 0..5 {
            children.push(Userset::ComputedUserset {
                relation: format!("req{}", i),
            });
        }

        let mut relations = vec![];
        for i in 0..5 {
            relations.push(RelationDefinition {
                name: format!("req{}", i),
                type_constraints: vec!["user".to_string()],
                rewrite: Userset::This,
            });
        }
        relations.push(RelationDefinition {
            name: "all_required".to_string(),
            type_constraints: vec![],
            rewrite: Userset::Intersection { children },
        });

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations,
                },
            )
            .await;

        // Add tuples for all requirements
        for i in 0..5 {
            tuple_reader
                .add_tuple(
                    "store1",
                    "document",
                    "doc1",
                    &format!("req{}", i),
                    "user",
                    "alice",
                    None,
                )
                .await;
        }

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "all_required".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(result.allowed, "All requirements met");
    }

    // ========== Section 5: Exclusion Relations (A but not B) ==========

    #[tokio::test]
    async fn test_exclusion_returns_true_if_base_true_and_subtract_false() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // can_view = viewer but not blocked
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "blocked".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "can_view".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::Exclusion {
                                base: Box::new(Userset::ComputedUserset {
                                    relation: "viewer".to_string(),
                                }),
                                subtract: Box::new(Userset::ComputedUserset {
                                    relation: "blocked".to_string(),
                                }),
                            },
                        },
                    ],
                },
            )
            .await;

        // Alice is viewer and NOT blocked
        tuple_reader
            .add_tuple(
                "store1", "document", "doc1", "viewer", "user", "alice", None,
            )
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "can_view".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(
            result.allowed,
            "Exclusion: viewer but not blocked should be allowed"
        );
    }

    #[tokio::test]
    async fn test_exclusion_returns_false_if_base_is_false() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "blocked".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "can_view".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::Exclusion {
                                base: Box::new(Userset::ComputedUserset {
                                    relation: "viewer".to_string(),
                                }),
                                subtract: Box::new(Userset::ComputedUserset {
                                    relation: "blocked".to_string(),
                                }),
                            },
                        },
                    ],
                },
            )
            .await;

        // Alice is NOT a viewer

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "can_view".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(!result.allowed, "Exclusion: not a viewer should be denied");
    }

    #[tokio::test]
    async fn test_exclusion_returns_false_if_subtract_is_true() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "blocked".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "can_view".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::Exclusion {
                                base: Box::new(Userset::ComputedUserset {
                                    relation: "viewer".to_string(),
                                }),
                                subtract: Box::new(Userset::ComputedUserset {
                                    relation: "blocked".to_string(),
                                }),
                            },
                        },
                    ],
                },
            )
            .await;

        // Alice is viewer BUT ALSO blocked
        tuple_reader
            .add_tuple(
                "store1", "document", "doc1", "viewer", "user", "alice", None,
            )
            .await;
        tuple_reader
            .add_tuple(
                "store1", "document", "doc1", "blocked", "user", "alice", None,
            )
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "can_view".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(
            !result.allowed,
            "Exclusion: viewer but blocked should be denied"
        );
    }

    #[tokio::test]
    async fn test_exclusion_returns_false_when_base_false_despite_subtract_cycle() {
        // Test that exclusion returns false (not error) when base is false,
        // even if subtract would cycle. Since base is false, we don't need subtract.
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // can_view = viewer but not blocked
        // blocked will have a cycle (blocked -> blocked_cyclic -> blocked)
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "blocked".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::ComputedUserset {
                                relation: "blocked_cyclic".to_string(),
                            },
                        },
                        RelationDefinition {
                            name: "blocked_cyclic".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::ComputedUserset {
                                relation: "blocked".to_string(),
                            },
                        },
                        RelationDefinition {
                            name: "can_view".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::Exclusion {
                                base: Box::new(Userset::ComputedUserset {
                                    relation: "viewer".to_string(),
                                }),
                                subtract: Box::new(Userset::ComputedUserset {
                                    relation: "blocked".to_string(),
                                }),
                            },
                        },
                    ],
                },
            )
            .await;

        // Alice is NOT a viewer (no tuple), so base=false
        // subtract would cycle, but we don't need it

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "can_view".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        // Should return false (base is false), not CycleDetected error
        let result = resolver.check(&request).await;
        assert!(
            result.is_ok(),
            "Exclusion with base=false should not error despite cyclic subtract: {:?}",
            result
        );
        assert!(
            !result.unwrap().allowed,
            "Exclusion with base=false should return false"
        );
    }

    #[tokio::test]
    async fn test_exclusion_returns_false_when_subtract_true_despite_base_cycle() {
        // Test that exclusion returns false (not error) when subtract is true,
        // even if base would cycle. Since subtract is true, result is false regardless of base.
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // can_view = viewer but not blocked
        // viewer will have a cycle (viewer -> viewer_cyclic -> viewer)
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::ComputedUserset {
                                relation: "viewer_cyclic".to_string(),
                            },
                        },
                        RelationDefinition {
                            name: "viewer_cyclic".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::ComputedUserset {
                                relation: "viewer".to_string(),
                            },
                        },
                        RelationDefinition {
                            name: "blocked".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "can_view".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::Exclusion {
                                base: Box::new(Userset::ComputedUserset {
                                    relation: "viewer".to_string(),
                                }),
                                subtract: Box::new(Userset::ComputedUserset {
                                    relation: "blocked".to_string(),
                                }),
                            },
                        },
                    ],
                },
            )
            .await;

        // Alice IS blocked (subtract=true), viewer would cycle but we don't need it
        tuple_reader
            .add_tuple(
                "store1", "document", "doc1", "blocked", "user", "alice", None,
            )
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "can_view".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        // Should return false (subtract is true), not CycleDetected error
        let result = resolver.check(&request).await;
        assert!(
            result.is_ok(),
            "Exclusion with subtract=true should not error despite cyclic base: {:?}",
            result
        );
        assert!(
            !result.unwrap().allowed,
            "Exclusion with subtract=true should return false"
        );
    }

    #[tokio::test]
    async fn test_exclusion_returns_cycle_error_when_both_branches_cycle() {
        // Test that exclusion returns CycleDetected when BOTH branches cycle
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // Both viewer and blocked have cycles
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::ComputedUserset {
                                relation: "viewer_cyclic".to_string(),
                            },
                        },
                        RelationDefinition {
                            name: "viewer_cyclic".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::ComputedUserset {
                                relation: "viewer".to_string(),
                            },
                        },
                        RelationDefinition {
                            name: "blocked".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::ComputedUserset {
                                relation: "blocked_cyclic".to_string(),
                            },
                        },
                        RelationDefinition {
                            name: "blocked_cyclic".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::ComputedUserset {
                                relation: "blocked".to_string(),
                            },
                        },
                        RelationDefinition {
                            name: "can_view".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::Exclusion {
                                base: Box::new(Userset::ComputedUserset {
                                    relation: "viewer".to_string(),
                                }),
                                subtract: Box::new(Userset::ComputedUserset {
                                    relation: "blocked".to_string(),
                                }),
                            },
                        },
                    ],
                },
            )
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "can_view".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        // Should return CycleDetected error since both branches cycle
        let result = resolver.check(&request).await;
        assert!(
            matches!(result, Err(DomainError::CycleDetected { .. })),
            "Exclusion with both cyclic branches should return CycleDetected, got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_exclusion_propagates_error_when_base_true_and_subtract_errors() {
        // Test that when base is true but subtract errors, the error is propagated
        // (because we need subtract's result to compute the final answer)
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec!["user".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "blocked".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::ComputedUserset {
                                relation: "blocked_cyclic".to_string(),
                            },
                        },
                        RelationDefinition {
                            name: "blocked_cyclic".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::ComputedUserset {
                                relation: "blocked".to_string(),
                            },
                        },
                        RelationDefinition {
                            name: "can_view".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::Exclusion {
                                base: Box::new(Userset::ComputedUserset {
                                    relation: "viewer".to_string(),
                                }),
                                subtract: Box::new(Userset::ComputedUserset {
                                    relation: "blocked".to_string(),
                                }),
                            },
                        },
                    ],
                },
            )
            .await;

        // Alice IS a viewer (base=true), but blocked cycles
        tuple_reader
            .add_tuple(
                "store1", "document", "doc1", "viewer", "user", "alice", None,
            )
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "can_view".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        // Should return CycleDetected error since we need subtract but it cycles
        let result = resolver.check(&request).await;
        assert!(
            matches!(result, Err(DomainError::CycleDetected { .. })),
            "Exclusion with base=true and cyclic subtract should return error, got {:?}",
            result
        );
    }

    // ========== Section 5b: Contextual Tuple Userset Resolution ==========

    #[tokio::test]
    async fn test_contextual_tuple_resolves_userset_reference() {
        // Test that contextual tuples with userset references like "team:eng#member"
        // are recursively resolved, not just matched literally.
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // Set up: document has viewer relation, team has member relation
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".to_string(), "team#member".to_string()],
                        rewrite: Userset::This,
                    }],
                },
            )
            .await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "team".to_string(),
                    relations: vec![RelationDefinition {
                        name: "member".to_string(),
                        type_constraints: vec!["user".to_string()],
                        rewrite: Userset::This,
                    }],
                },
            )
            .await;

        // Alice is a member of team:eng (stored tuple)
        tuple_reader
            .add_tuple("store1", "team", "eng", "member", "user", "alice", None)
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        // Use contextual tuple to grant "team:eng#member" viewer access to document:readme
        // This should resolve recursively: alice -> team:eng#member -> viewer of document:readme
        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            contextual_tuples: Arc::new(vec![ContextualTuple {
                user: "team:eng#member".to_string(),
                relation: "viewer".to_string(),
                object: "document:readme".to_string(),
            }]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(
            result.allowed,
            "Contextual tuple with userset reference should be recursively resolved"
        );
    }

    #[tokio::test]
    async fn test_contextual_tuple_userset_not_member_denied() {
        // Test that users who are NOT members of the userset are denied
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".to_string(), "team#member".to_string()],
                        rewrite: Userset::This,
                    }],
                },
            )
            .await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "team".to_string(),
                    relations: vec![RelationDefinition {
                        name: "member".to_string(),
                        type_constraints: vec!["user".to_string()],
                        rewrite: Userset::This,
                    }],
                },
            )
            .await;

        // Alice is a member of team:eng, but Bob is NOT
        tuple_reader
            .add_tuple("store1", "team", "eng", "member", "user", "alice", None)
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        // Use contextual tuple to grant "team:eng#member" viewer access
        // Bob should be denied because he's not a member of team:eng
        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:bob".to_string(),
            relation: "viewer".to_string(),
            object: "document:readme".to_string(),
            contextual_tuples: Arc::new(vec![ContextualTuple {
                user: "team:eng#member".to_string(),
                relation: "viewer".to_string(),
                object: "document:readme".to_string(),
            }]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(
            !result.allowed,
            "User not in userset should be denied even with contextual tuple"
        );
    }

    // ========== Section 6: Safety Mechanisms ==========

    #[tokio::test]
    async fn test_depth_limit_prevents_stack_overflow() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // Create deep chain: level0.viewer -> level1.viewer -> ... -> levelN.viewer
        for i in 0..30 {
            let type_name = format!("level{}", i);
            let rewrite = if i == 0 {
                Userset::This
            } else {
                Userset::TupleToUserset {
                    tupleset: "parent".to_string(),
                    computed_userset: "viewer".to_string(),
                }
            };

            let mut relations = vec![RelationDefinition {
                name: "viewer".to_string(),
                type_constraints: vec!["user".to_string()],
                rewrite,
            }];

            if i > 0 {
                relations.push(RelationDefinition {
                    name: "parent".to_string(),
                    type_constraints: vec![format!("level{}", i - 1)],
                    rewrite: Userset::This,
                });
            }

            model_reader
                .add_type(
                    "store1",
                    TypeDefinition {
                        type_name,
                        relations,
                    },
                )
                .await;
        }

        // Create chain of parent relationships
        for i in 1..30 {
            tuple_reader
                .add_tuple(
                    "store1",
                    &format!("level{}", i),
                    "obj",
                    "parent",
                    &format!("level{}", i - 1),
                    "obj",
                    None,
                )
                .await;
        }

        // Add viewer at level0
        tuple_reader
            .add_tuple("store1", "level0", "obj", "viewer", "user", "alice", None)
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        // Try to check at level29 (should exceed depth limit of 25)
        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "level29:obj".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await;
        assert!(
            matches!(result, Err(DomainError::DepthLimitExceeded { .. })),
            "Should return depth limit exceeded error"
        );
    }

    #[tokio::test]
    async fn test_depth_limit_matches_openfga_default() {
        let config = ResolverConfig::default();
        assert_eq!(config.max_depth, 25, "Default depth limit should be 25");
    }

    #[tokio::test]
    async fn test_returns_depth_limit_exceeded_error() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // Create chain that exceeds depth limit
        for i in 0..30 {
            let type_name = format!("type{}", i);
            model_reader
                .add_type(
                    "store1",
                    TypeDefinition {
                        type_name: type_name.clone(),
                        relations: vec![
                            RelationDefinition {
                                name: "parent".to_string(),
                                type_constraints: vec![],
                                rewrite: Userset::This,
                            },
                            RelationDefinition {
                                name: "viewer".to_string(),
                                type_constraints: vec![],
                                rewrite: if i == 0 {
                                    Userset::This
                                } else {
                                    Userset::TupleToUserset {
                                        tupleset: "parent".to_string(),
                                        computed_userset: "viewer".to_string(),
                                    }
                                },
                            },
                        ],
                    },
                )
                .await;
        }

        for i in 1..30 {
            tuple_reader
                .add_tuple(
                    "store1",
                    &format!("type{}", i),
                    "obj",
                    "parent",
                    &format!("type{}", i - 1),
                    "obj",
                    None,
                )
                .await;
        }

        tuple_reader
            .add_tuple("store1", "type0", "obj", "viewer", "user", "alice", None)
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "type29:obj".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await;
        match result {
            Err(DomainError::DepthLimitExceeded { max_depth }) => {
                assert_eq!(max_depth, 25);
            }
            _ => panic!("Expected DepthLimitExceeded error"),
        }
    }

    #[tokio::test]
    async fn test_cycle_detection_prevents_infinite_loops() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // Create a cycle: doc.viewer -> folder.viewer -> doc.viewer
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "folder".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "contained_doc".to_string(),
                            type_constraints: vec!["document".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::TupleToUserset {
                                tupleset: "contained_doc".to_string(),
                                computed_userset: "viewer".to_string(),
                            },
                        },
                    ],
                },
            )
            .await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "parent".to_string(),
                            type_constraints: vec!["folder".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "viewer".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::TupleToUserset {
                                tupleset: "parent".to_string(),
                                computed_userset: "viewer".to_string(),
                            },
                        },
                    ],
                },
            )
            .await;

        // Create circular reference
        tuple_reader
            .add_tuple(
                "store1", "document", "doc1", "parent", "folder", "folder1", None,
            )
            .await;
        tuple_reader
            .add_tuple(
                "store1",
                "folder",
                "folder1",
                "contained_doc",
                "document",
                "doc1",
                None,
            )
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await;
        // Should either detect cycle or hit depth limit, but not infinite loop
        assert!(
            matches!(
                result,
                Err(DomainError::CycleDetected { .. })
                    | Err(DomainError::DepthLimitExceeded { .. })
            ),
            "Should detect cycle or hit depth limit"
        );
    }

    #[tokio::test]
    async fn test_cycle_detection_doesnt_false_positive_on_valid_dags() {
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // Create a valid DAG where same object is reachable via multiple paths
        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "org".to_string(),
                    relations: vec![RelationDefinition {
                        name: "admin".to_string(),
                        type_constraints: vec!["user".to_string()],
                        rewrite: Userset::This,
                    }],
                },
            )
            .await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "folder".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "parent".to_string(),
                            type_constraints: vec!["org".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "admin".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::TupleToUserset {
                                tupleset: "parent".to_string(),
                                computed_userset: "admin".to_string(),
                            },
                        },
                    ],
                },
            )
            .await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![
                        RelationDefinition {
                            name: "parent".to_string(),
                            type_constraints: vec!["folder".to_string()],
                            rewrite: Userset::This,
                        },
                        RelationDefinition {
                            name: "admin".to_string(),
                            type_constraints: vec![],
                            rewrite: Userset::TupleToUserset {
                                tupleset: "parent".to_string(),
                                computed_userset: "admin".to_string(),
                            },
                        },
                    ],
                },
            )
            .await;

        // Set up: org1 -> folder1, folder2 -> doc1
        tuple_reader
            .add_tuple("store1", "org", "org1", "admin", "user", "alice", None)
            .await;
        tuple_reader
            .add_tuple("store1", "folder", "folder1", "parent", "org", "org1", None)
            .await;
        tuple_reader
            .add_tuple("store1", "folder", "folder2", "parent", "org", "org1", None)
            .await;
        tuple_reader
            .add_tuple(
                "store1", "document", "doc1", "parent", "folder", "folder1", None,
            )
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "admin".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(result.allowed, "Valid DAG should allow access");
    }

    #[tokio::test]
    async fn test_timeout_is_configurable() {
        let config = ResolverConfig {
            max_depth: 25,
            timeout: Duration::from_millis(100),
        };

        assert_eq!(config.timeout, Duration::from_millis(100));
    }

    #[tokio::test]
    async fn test_returns_timeout_error_with_context() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::sync::Mutex;

        // Create a slow tuple reader that delays responses
        struct SlowTupleReader {
            delay: Duration,
            stores: Mutex<HashSet<String>>,
            started: AtomicBool,
        }

        impl SlowTupleReader {
            fn new(delay: Duration) -> Self {
                Self {
                    delay,
                    stores: Mutex::new(HashSet::new()),
                    started: AtomicBool::new(false),
                }
            }

            async fn add_store(&self, store_id: &str) {
                self.stores.lock().await.insert(store_id.to_string());
            }
        }

        #[async_trait]
        impl TupleReader for SlowTupleReader {
            async fn read_tuples(
                &self,
                _store_id: &str,
                _object_type: &str,
                _object_id: &str,
                _relation: &str,
            ) -> DomainResult<Vec<StoredTupleRef>> {
                self.started.store(true, Ordering::SeqCst);
                tokio::time::sleep(self.delay).await;
                Ok(vec![])
            }

            async fn store_exists(&self, store_id: &str) -> DomainResult<bool> {
                Ok(self.stores.lock().await.contains(store_id))
            }
        }

        let tuple_reader = Arc::new(SlowTupleReader::new(Duration::from_secs(10)));
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".to_string()],
                        rewrite: Userset::This,
                    }],
                },
            )
            .await;

        let config = ResolverConfig {
            max_depth: 25,
            timeout: Duration::from_millis(50),
        };

        let resolver = GraphResolver::with_config(tuple_reader.clone(), model_reader, config);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await;
        match result {
            Err(DomainError::Timeout { duration_ms }) => {
                assert_eq!(duration_ms, 50);
            }
            _ => panic!("Expected Timeout error, got {:?}", result),
        }
    }

    // ========== Section 6b: Edge Case Tests ==========

    #[tokio::test]
    async fn test_empty_union_returns_false() {
        // Empty union should return false (no branches to satisfy)
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Union { children: vec![] },
                    }],
                },
            )
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(!result.allowed, "Empty union should return false");
    }

    #[tokio::test]
    async fn test_empty_intersection_returns_true() {
        // Empty intersection should return true (vacuously all conditions met)
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec![],
                        rewrite: Userset::Intersection { children: vec![] },
                    }],
                },
            )
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(result.allowed, "Empty intersection should return true");
    }

    #[tokio::test]
    async fn test_depth_limit_at_boundary_24_succeeds() {
        // Test that depth 24 (just under limit of 25) succeeds
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        // Create chain of exactly 24 levels
        for i in 0..25 {
            let type_name = format!("level{}", i);
            let rewrite = if i == 0 {
                Userset::This
            } else {
                Userset::TupleToUserset {
                    tupleset: "parent".to_string(),
                    computed_userset: "viewer".to_string(),
                }
            };

            let mut relations = vec![RelationDefinition {
                name: "viewer".to_string(),
                type_constraints: vec!["user".to_string()],
                rewrite,
            }];

            if i > 0 {
                relations.push(RelationDefinition {
                    name: "parent".to_string(),
                    type_constraints: vec![format!("level{}", i - 1)],
                    rewrite: Userset::This,
                });
            }

            model_reader
                .add_type(
                    "store1",
                    TypeDefinition {
                        type_name,
                        relations,
                    },
                )
                .await;
        }

        // Create chain of parent relationships (24 hops)
        for i in 1..25 {
            tuple_reader
                .add_tuple(
                    "store1",
                    &format!("level{}", i),
                    "obj",
                    "parent",
                    &format!("level{}", i - 1),
                    "obj",
                    None,
                )
                .await;
        }

        // Alice is viewer at level0
        tuple_reader
            .add_tuple("store1", "level0", "obj", "viewer", "user", "alice", None)
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        // Check at level24 - should succeed (depth 24 is within limit of 25)
        let request = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "level24:obj".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request).await.unwrap();
        assert!(
            result.allowed,
            "Depth 24 should succeed (within limit of 25)"
        );
    }

    #[tokio::test]
    async fn test_contextual_tuple_overrides_stored_tuple() {
        // Contextual tuples should be checked first and can grant access
        // even if no stored tuple exists
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".to_string()],
                        rewrite: Userset::This,
                    }],
                },
            )
            .await;

        // No stored tuple for alice

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        // Without contextual tuple - should be denied
        let request_without = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result = resolver.check(&request_without).await.unwrap();
        assert!(!result.allowed, "Should be denied without contextual tuple");

        // With contextual tuple - should be allowed
        let request_with = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![ContextualTuple {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:doc1".to_string(),
            }]),
        };

        let result = resolver.check(&request_with).await.unwrap();
        assert!(result.allowed, "Contextual tuple should grant access");
    }

    #[tokio::test]
    async fn test_contextual_tuple_does_not_conflict_with_stored() {
        // Both contextual and stored tuples can coexist
        let tuple_reader = Arc::new(MockTupleReader::new());
        let model_reader = Arc::new(MockModelReader::new());

        tuple_reader.add_store("store1").await;

        model_reader
            .add_type(
                "store1",
                TypeDefinition {
                    type_name: "document".to_string(),
                    relations: vec![RelationDefinition {
                        name: "viewer".to_string(),
                        type_constraints: vec!["user".to_string()],
                        rewrite: Userset::This,
                    }],
                },
            )
            .await;

        // Bob has stored tuple
        tuple_reader
            .add_tuple("store1", "document", "doc1", "viewer", "user", "bob", None)
            .await;

        let resolver = GraphResolver::new(tuple_reader, model_reader);

        // Alice uses contextual tuple, Bob uses stored - both should work
        let request_alice = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:alice".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![ContextualTuple {
                user: "user:alice".to_string(),
                relation: "viewer".to_string(),
                object: "document:doc1".to_string(),
            }]),
        };

        let request_bob = CheckRequest {
            store_id: "store1".to_string(),
            user: "user:bob".to_string(),
            relation: "viewer".to_string(),
            object: "document:doc1".to_string(),
            contextual_tuples: Arc::new(vec![]),
        };

        let result_alice = resolver.check(&request_alice).await.unwrap();
        let result_bob = resolver.check(&request_bob).await.unwrap();

        assert!(
            result_alice.allowed,
            "Alice should have access via contextual tuple"
        );
        assert!(
            result_bob.allowed,
            "Bob should have access via stored tuple"
        );
    }

    // ========== Section 7: Property-Based Tests ==========

    use proptest::prelude::*;

    /// Strategy to generate valid user identifiers
    fn user_strategy() -> impl Strategy<Value = String> {
        "[a-z]{1,10}".prop_map(|s| format!("user:{}", s))
    }

    /// Strategy to generate valid object identifiers
    fn object_strategy() -> impl Strategy<Value = (String, String, String)> {
        ("[a-z]{1,10}", "[a-z0-9]{1,10}").prop_map(|(type_name, id)| {
            (
                type_name.clone(),
                id.clone(),
                format!("{}:{}", type_name, id),
            )
        })
    }

    /// Strategy to generate valid relation names
    fn relation_strategy() -> impl Strategy<Value = String> {
        "[a-z]{1,10}"
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        /// Property: Resolver never panics on any input
        #[test]
        fn test_property_resolver_never_panics(
            user in user_strategy(),
            (type_name, object_id, object) in object_strategy(),
            relation in relation_strategy(),
            store_id in "[a-z]{1,5}",
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let tuple_reader = Arc::new(MockTupleReader::new());
                let model_reader = Arc::new(MockModelReader::new());

                tuple_reader.add_store(&store_id).await;

                // Add type definition
                model_reader
                    .add_type(
                        &store_id,
                        TypeDefinition {
                            type_name: type_name.clone(),
                            relations: vec![RelationDefinition {
                                name: relation.clone(),
                                type_constraints: vec!["user".to_string()],
                                rewrite: Userset::This,
                            }],
                        },
                    )
                    .await;

                // Optionally add a tuple
                tuple_reader
                    .add_tuple(&store_id, &type_name, &object_id, &relation, "user", user.strip_prefix("user:").unwrap_or(&user), None)
                    .await;

                let resolver = GraphResolver::new(tuple_reader, model_reader);

                let request = CheckRequest {
                    store_id: store_id.clone(),
                    user: user.clone(),
                    relation: relation.clone(),
                    object: object.clone(),
                    contextual_tuples: Arc::new(vec![]),
                };

                // Should not panic - may return Ok or Err, but should never panic
                let _ = resolver.check(&request).await;
            });
        }

        /// Property: Resolver always terminates (no infinite loops)
        /// Verified by using a short timeout and ensuring we always get a result
        #[test]
        fn test_property_resolver_always_terminates(
            user in user_strategy(),
            (type_name, _object_id, object) in object_strategy(),
            relation in relation_strategy(),
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let terminated = rt.block_on(async {
                let tuple_reader = Arc::new(MockTupleReader::new());
                let model_reader = Arc::new(MockModelReader::new());

                tuple_reader.add_store("store1").await;

                // Add type definition with potential cycle
                model_reader
                    .add_type(
                        "store1",
                        TypeDefinition {
                            type_name: type_name.clone(),
                            relations: vec![RelationDefinition {
                                name: relation.clone(),
                                type_constraints: vec!["user".to_string()],
                                rewrite: Userset::This,
                            }],
                        },
                    )
                    .await;

                let config = ResolverConfig {
                    max_depth: 25,
                    timeout: Duration::from_secs(1), // Short timeout
                };

                let resolver = GraphResolver::with_config(tuple_reader, model_reader, config);

                let request = CheckRequest {
                    store_id: "store1".to_string(),
                    user: user.clone(),
                    relation: relation.clone(),
                    object: object.clone(),
                    contextual_tuples: Arc::new(vec![]),
                };

                // Use a timeout to ensure termination
                let result = tokio::time::timeout(
                    Duration::from_secs(2),
                    resolver.check(&request),
                )
                .await;

                // Should always terminate within 2 seconds
                result.is_ok()
            });
            prop_assert!(terminated, "Resolver should always terminate");
        }

        /// Property: Resolver returns correct result for known graphs (direct tuples)
        #[test]
        fn test_property_resolver_returns_correct_result_for_known_graphs(
            user_name in "[a-z]{1,5}",
            object_id in "[a-z0-9]{1,5}",
            has_tuple in proptest::bool::ANY,
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let (allowed, expected) = rt.block_on(async {
                let tuple_reader = Arc::new(MockTupleReader::new());
                let model_reader = Arc::new(MockModelReader::new());

                tuple_reader.add_store("store1").await;

                model_reader
                    .add_type(
                        "store1",
                        TypeDefinition {
                            type_name: "document".to_string(),
                            relations: vec![RelationDefinition {
                                name: "viewer".to_string(),
                                type_constraints: vec!["user".to_string()],
                                rewrite: Userset::This,
                            }],
                        },
                    )
                    .await;

                if has_tuple {
                    tuple_reader
                        .add_tuple("store1", "document", &object_id, "viewer", "user", &user_name, None)
                        .await;
                }

                let resolver = GraphResolver::new(tuple_reader, model_reader);

                let request = CheckRequest {
                    store_id: "store1".to_string(),
                    user: format!("user:{}", user_name),
                    relation: "viewer".to_string(),
                    object: format!("document:{}", object_id),
                    contextual_tuples: Arc::new(vec![]),
                };

                let result = resolver.check(&request).await.unwrap();
                (result.allowed, has_tuple)
            });
            // Direct tuple: result should match whether tuple exists
            prop_assert_eq!(allowed, expected,
                "Result should match tuple existence: tuple={}, allowed={}",
                expected, allowed);
        }

        /// Property: Adding a tuple never makes check false→true incorrectly
        /// (Monotonicity: adding permissions should only grant more access)
        #[test]
        fn test_property_adding_tuple_never_incorrectly_grants(
            user_name in "[a-z]{1,5}",
            object_id in "[a-z0-9]{1,5}",
            other_user in "[a-z]{1,5}",
        ) {
            // Skip if users are the same
            prop_assume!(user_name != other_user);

            let rt = tokio::runtime::Runtime::new().unwrap();
            let (before, after) = rt.block_on(async {
                let tuple_reader = Arc::new(MockTupleReader::new());
                let model_reader = Arc::new(MockModelReader::new());

                tuple_reader.add_store("store1").await;

                model_reader
                    .add_type(
                        "store1",
                        TypeDefinition {
                            type_name: "document".to_string(),
                            relations: vec![RelationDefinition {
                                name: "viewer".to_string(),
                                type_constraints: vec!["user".to_string()],
                                rewrite: Userset::This,
                            }],
                        },
                    )
                    .await;

                let resolver = GraphResolver::new(tuple_reader.clone(), model_reader);

                // Check for user before adding tuple for OTHER user
                let request = CheckRequest {
                    store_id: "store1".to_string(),
                    user: format!("user:{}", user_name),
                    relation: "viewer".to_string(),
                    object: format!("document:{}", object_id),
                    contextual_tuples: Arc::new(vec![]),
                };

                let result_before = resolver.check(&request).await.unwrap();

                // Add tuple for OTHER user
                tuple_reader
                    .add_tuple("store1", "document", &object_id, "viewer", "user", &other_user, None)
                    .await;

                let result_after = resolver.check(&request).await.unwrap();
                (result_before.allowed, result_after.allowed)
            });
            // Adding a tuple for a DIFFERENT user should not change our user's access
            prop_assert_eq!(before, after,
                "Adding tuple for other_user={} should not affect user={}'s access. Before={}, After={}",
                other_user, user_name, before, after);
        }

        /// Property: Deleting a tuple never makes check true→false incorrectly
        /// (Only the specific permission should be revoked)
        #[test]
        fn test_property_deleting_tuple_only_revokes_specific_permission(
            user_name in "[a-z]{1,5}",
            object_id in "[a-z0-9]{1,5}",
            other_object in "[a-z0-9]{1,5}",
        ) {
            // Skip if objects are the same
            prop_assume!(object_id != other_object);

            let rt = tokio::runtime::Runtime::new().unwrap();
            let (before, after) = rt.block_on(async {
                let tuple_reader = Arc::new(MockTupleReader::new());
                let model_reader = Arc::new(MockModelReader::new());

                tuple_reader.add_store("store1").await;

                model_reader
                    .add_type(
                        "store1",
                        TypeDefinition {
                            type_name: "document".to_string(),
                            relations: vec![RelationDefinition {
                                name: "viewer".to_string(),
                                type_constraints: vec!["user".to_string()],
                                rewrite: Userset::This,
                            }],
                        },
                    )
                    .await;

                // Add tuples for both objects
                tuple_reader
                    .add_tuple("store1", "document", &object_id, "viewer", "user", &user_name, None)
                    .await;
                tuple_reader
                    .add_tuple("store1", "document", &other_object, "viewer", "user", &user_name, None)
                    .await;

                let resolver = GraphResolver::new(tuple_reader.clone(), model_reader);

                // Check access to other_object before deleting tuple for object_id
                let request = CheckRequest {
                    store_id: "store1".to_string(),
                    user: format!("user:{}", user_name),
                    relation: "viewer".to_string(),
                    object: format!("document:{}", other_object),
                    contextual_tuples: Arc::new(vec![]),
                };

                let result_before = resolver.check(&request).await.unwrap();

                // Delete tuple for object_id (not other_object)
                tuple_reader
                    .remove_tuple("store1", "document", &object_id, "viewer", "user", &user_name)
                    .await;

                // Access to other_object should still be true
                let result_after = resolver.check(&request).await.unwrap();
                (result_before.allowed, result_after.allowed)
            });
            prop_assert!(before, "Should have access before");
            prop_assert!(after,
                "Deleting tuple for object_id={} should not affect access to other_object={}",
                object_id, other_object);
        }
    }
}
