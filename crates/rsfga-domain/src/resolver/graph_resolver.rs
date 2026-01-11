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

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::time::timeout;

use crate::cel::{global_cache, CelContext, CelValue};
use crate::error::{DomainError, DomainResult};
use crate::model::{TypeConstraint, Userset};

use super::config::ResolverConfig;
use super::context::TraversalContext;
use super::traits::{ModelReader, TupleReader};
use super::types::{CheckRequest, CheckResult};

/// Type alias for boxed future to handle async recursion.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Graph resolver for permission checks.
///
/// Performs async graph traversal to determine if a user has
/// a specific permission on an object.
pub struct GraphResolver<T, M> {
    tuple_reader: Arc<T>,
    model_reader: Arc<M>,
    config: ResolverConfig,
}

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
            self.resolve_userset(
                request,
                relation_def.rewrite,
                relation_def.type_constraints,
                object_type,
                object_id,
                ctx,
            )
            .await
        })
    }

    /// Resolves a userset rewrite (boxed for recursion).
    fn resolve_userset(
        &self,
        request: CheckRequest,
        userset: Userset,
        type_constraints: Vec<TypeConstraint>,
        object_type: String,
        object_id: String,
        ctx: TraversalContext,
    ) -> BoxFuture<'_, DomainResult<CheckResult>> {
        Box::pin(async move {
            match userset {
                Userset::This => {
                    // Direct tuple lookup with type constraint validation
                    self.resolve_direct(request, type_constraints, object_type, object_id, ctx)
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
                        context: request.context,
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
                    self.resolve_union(
                        request,
                        children,
                        type_constraints,
                        object_type,
                        object_id,
                        ctx,
                    )
                    .await
                }

                Userset::Intersection { children } => {
                    // All children must be true (parallel execution with short-circuit)
                    self.resolve_intersection(
                        request,
                        children,
                        type_constraints,
                        object_type,
                        object_id,
                        ctx,
                    )
                    .await
                }

                Userset::Exclusion { base, subtract } => {
                    // Base must be true AND subtract must be false
                    self.resolve_exclusion(
                        request,
                        *base,
                        *subtract,
                        type_constraints,
                        object_type,
                        object_id,
                        ctx,
                    )
                    .await
                }
            }
        })
    }

    /// Resolves a direct tuple assignment.
    ///
    /// Security: Validates that tuple user types match the relation's type_constraints.
    /// This prevents unauthorized user types from gaining access (e.g., bot:scraper
    /// accessing resources only intended for user or group#member types).
    fn resolve_direct(
        &self,
        request: CheckRequest,
        type_constraints: Vec<TypeConstraint>,
        object_type: String,
        object_id: String,
        ctx: TraversalContext,
    ) -> BoxFuture<'_, DomainResult<CheckResult>> {
        Box::pin(async move {
            // First check contextual tuples
            for ct in request.contextual_tuples.iter() {
                if ct.object == request.object && ct.relation == request.relation {
                    // Security: Validate type constraints for contextual tuples
                    // Skip if type_constraints is empty (relation allows any type)
                    if !type_constraints.is_empty()
                        && !self.user_matches_type_constraints(&ct.user, &type_constraints)
                    {
                        continue; // Skip this tuple, type not allowed
                    }

                    // Check for direct match or type wildcard
                    if self.user_matches(&request.user, &ct.user) {
                        // Evaluate condition if present
                        let condition_ok = self
                            .evaluate_condition(
                                &request.store_id,
                                ct.condition_name.as_deref(),
                                ct.condition_context.as_ref(),
                                &request.context,
                            )
                            .await?;
                        if condition_ok {
                            return Ok(CheckResult { allowed: true });
                        }
                        // Condition failed, continue checking other tuples
                        continue;
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
                            context: request.context.clone(),
                        };

                        let new_ctx = ctx.increment_depth();
                        let result = self.resolve_check(userset_request, new_ctx.clone()).await?;
                        if result.allowed {
                            // Evaluate condition if present
                            let condition_ok = self
                                .evaluate_condition(
                                    &request.store_id,
                                    ct.condition_name.as_deref(),
                                    ct.condition_context.as_ref(),
                                    &request.context,
                                )
                                .await?;
                            if condition_ok {
                                return Ok(CheckResult { allowed: true });
                            }
                            // Condition failed, continue checking other tuples
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
                // Security: Validate type constraints for stored tuples
                // Skip if type_constraints is empty (relation allows any type)
                if !type_constraints.is_empty() {
                    let tuple_type_ref = if let Some(ref rel) = tuple.user_relation {
                        format!("{}#{}", tuple.user_type, rel)
                    } else {
                        tuple.user_type.clone()
                    };

                    if !self.type_matches_constraints(&tuple_type_ref, &type_constraints) {
                        continue; // Skip this tuple, type not allowed
                    }
                }

                let tuple_user = if let Some(ref rel) = tuple.user_relation {
                    format!("{}:{}#{}", tuple.user_type, tuple.user_id, rel)
                } else {
                    format!("{}:{}", tuple.user_type, tuple.user_id)
                };

                // Check for direct match or wildcard match (handled by user_matches)
                if self.user_matches(&request.user, &tuple_user) {
                    // Evaluate condition if present
                    let condition_ok = self
                        .evaluate_condition(
                            &request.store_id,
                            tuple.condition_name.as_deref(),
                            tuple.condition_context.as_ref(),
                            &request.context,
                        )
                        .await?;
                    if condition_ok {
                        return Ok(CheckResult { allowed: true });
                    }
                    // Condition failed, continue checking other tuples
                    continue;
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
                        context: request.context.clone(),
                    };

                    let new_ctx = ctx.increment_depth();
                    let result = self.resolve_check(userset_request, new_ctx).await?;
                    if result.allowed {
                        // Evaluate condition if present
                        let condition_ok = self
                            .evaluate_condition(
                                &request.store_id,
                                tuple.condition_name.as_deref(),
                                tuple.condition_context.as_ref(),
                                &request.context,
                            )
                            .await?;
                        if condition_ok {
                            return Ok(CheckResult { allowed: true });
                        }
                        // Condition failed, continue checking other tuples
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
                context: request.context.clone(),
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
        type_constraints: Vec<TypeConstraint>,
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
                    type_constraints.clone(),
                    object_type.clone(),
                    object_id.clone(),
                    new_ctx.clone(),
                )
            })
            .collect();

        // Track errors for reporting if all branches fail
        // Path-termination errors (CycleDetected, DepthLimitExceeded) mean "this branch
        // couldn't find access" - they should be treated as false for union semantics,
        // not as fatal errors. Only propagate if ALL branches hit path-termination.
        let mut fatal_error: Option<DomainError> = None;
        let mut path_termination_error: Option<DomainError> = None;
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
                    // Path-termination errors (cycle, depth limit) are NOT fatal for unions.
                    // They mean "this branch couldn't reach a conclusion" which is effectively
                    // "this branch didn't grant access" - so treat as false and continue.
                    if matches!(
                        e,
                        DomainError::CycleDetected { .. } | DomainError::DepthLimitExceeded { .. }
                    ) {
                        path_termination_error = Some(e);
                    } else {
                        fatal_error = Some(e);
                    }
                }
            }
        }

        // Fatal errors (storage errors, etc.) should always propagate
        if let Some(e) = fatal_error {
            return Err(e);
        }

        // If ALL branches returned path-termination errors (no false results), propagate
        // the error - this indicates the entire union couldn't be evaluated
        if !had_false_result {
            if let Some(e) = path_termination_error {
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
        type_constraints: Vec<TypeConstraint>,
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
                    type_constraints.clone(),
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
    #[allow(clippy::too_many_arguments)]
    async fn resolve_exclusion(
        &self,
        request: CheckRequest,
        base: Userset,
        subtract: Userset,
        type_constraints: Vec<TypeConstraint>,
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
                type_constraints.clone(),
                object_type.clone(),
                object_id.clone(),
                new_ctx.clone(),
            ),
            self.resolve_userset(
                request,
                subtract,
                type_constraints,
                object_type,
                object_id,
                new_ctx,
            ),
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
    ///
    /// Security: Wildcards are only valid in tuple_user (stored tuples), never in
    /// requesting_user. A user cannot request with "admin:*" to match stored tuples.
    fn user_matches(&self, requesting_user: &str, tuple_user: &str) -> bool {
        // Security: Reject wildcards in requesting_user to prevent authorization bypass
        // A user should never be able to request with "admin:*" or "user:*"
        if requesting_user.ends_with(":*") {
            return false;
        }

        if requesting_user == tuple_user {
            return true;
        }

        // Check for type wildcard in tuple_user (e.g., user:* matches user:alice)
        // This allows stored tuples like (user:*, viewer, document:readme) to grant
        // access to all users of that type
        if tuple_user.ends_with(":*") {
            if let Some((tuple_type, _)) = tuple_user.split_once(':') {
                if let Some((user_type, _)) = requesting_user.split_once(':') {
                    return tuple_type == user_type;
                }
            }
        }

        false
    }

    /// Checks if a user string (e.g., "user:alice" or "group:eng#member") matches
    /// any of the type constraints.
    ///
    /// Type constraints are strings like:
    /// - "user" - direct user type
    /// - "group#member" - userset reference
    fn user_matches_type_constraints(
        &self,
        user: &str,
        type_constraints: &[TypeConstraint],
    ) -> bool {
        // Parse user to get type (and optional relation for userset references)
        let (user_type, user_relation) = if let Some((obj, rel)) = user.split_once('#') {
            // Userset reference like "group:eng#member" -> type="group", relation="member"
            if let Some((type_part, _)) = obj.split_once(':') {
                (type_part, Some(rel))
            } else {
                return false; // Invalid format
            }
        } else if let Some((type_part, _)) = user.split_once(':') {
            // Direct user like "user:alice" -> type="user", relation=None
            (type_part, None)
        } else {
            return false; // Invalid format
        };

        // Check if this type matches any constraint
        self.type_matches_constraints_internal(user_type, user_relation, type_constraints)
    }

    /// Checks if a tuple type reference (e.g., "user" or "group#member") matches
    /// any of the type constraints.
    fn type_matches_constraints(
        &self,
        type_ref: &str,
        type_constraints: &[TypeConstraint],
    ) -> bool {
        // type_ref is either "user" or "group#member"
        let (user_type, user_relation) = if let Some((type_part, rel)) = type_ref.split_once('#') {
            (type_part, Some(rel))
        } else {
            (type_ref, None)
        };

        self.type_matches_constraints_internal(user_type, user_relation, type_constraints)
    }

    /// Internal helper to check type constraints.
    fn type_matches_constraints_internal(
        &self,
        user_type: &str,
        user_relation: Option<&str>,
        type_constraints: &[TypeConstraint],
    ) -> bool {
        for constraint in type_constraints {
            let type_name = &constraint.type_name;
            if let Some((constraint_type, constraint_rel)) = type_name.split_once('#') {
                // Constraint is a userset reference like "group#member"
                if user_type == constraint_type && user_relation == Some(constraint_rel) {
                    return true;
                }
            } else {
                // Constraint is a direct type like "user"
                if user_type == type_name && user_relation.is_none() {
                    return true;
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

    /// Evaluates a condition for a tuple.
    ///
    /// This is the **check-time** validation for conditions. Condition existence
    /// is verified here rather than at tuple write-time, which allows:
    /// - Tuples to reference conditions not yet defined in the model
    /// - Flexible deployment ordering (tuples before model updates)
    /// - Model updates without requiring tuple rewrites
    ///
    /// See also: `rsfga_storage::traits::validate_tuple` for write-time validation.
    ///
    /// # Returns
    ///
    /// - `Ok(true)` if the tuple has no condition, or the condition evaluates to true
    /// - `Ok(false)` if the condition evaluates to false
    /// - `Err` if the condition is not found in the model, or the expression fails
    ///
    /// # Context Precedence
    ///
    /// When building the CEL evaluation context, tuple context takes precedence
    /// over request context. See the context merging logic below for details.
    async fn evaluate_condition(
        &self,
        store_id: &str,
        condition_name: Option<&str>,
        tuple_condition_context: Option<&HashMap<String, serde_json::Value>>,
        request_context: &HashMap<String, serde_json::Value>,
    ) -> DomainResult<bool> {
        // If no condition, tuple grants access unconditionally
        let condition_name = match condition_name {
            Some(name) => name,
            None => return Ok(true),
        };

        // Get the authorization model to find the condition definition
        let model = self.model_reader.get_model(store_id).await?;
        let condition =
            model
                .find_condition(condition_name)
                .ok_or_else(|| DomainError::ResolverError {
                    message: format!("condition not found: {}", condition_name),
                })?;

        // Get the parsed CEL expression from cache (or parse and cache it)
        // PERFORMANCE FIX: CEL expressions are now cached to avoid repeated parsing.
        // See: https://github.com/julianshen/rsfga/issues/82
        let expr = global_cache()
            .get_or_parse(&condition.expression)
            .map_err(|e| DomainError::ResolverError {
                message: format!(
                    "failed to parse condition expression '{}': {}",
                    condition.expression, e
                ),
            })?;

        // Build the CEL context
        let mut cel_ctx = CelContext::new();

        // ==========================================================================
        // SECURITY-CRITICAL: Context Merging with Tuple Precedence
        // ==========================================================================
        //
        // Build the "context" map from request context + tuple condition context.
        // Per OpenFGA spec: tuple condition context takes precedence over request context.
        //
        // WHY THIS MATTERS FOR SECURITY:
        //
        // When a tuple is written with condition context (e.g., {"max_amount": 1000}),
        // this establishes a trust boundary. If request context could override this,
        // a malicious caller could bypass the intended restriction by passing
        // {"max_amount": 999999} in the request.
        //
        // Example attack prevented by this design:
        //   - Admin writes tuple: (user:alice, can_transfer, account:corp, {max_amount: 1000})
        //   - Malicious caller sends check with context: {max_amount: 999999}
        //   - If request took precedence, alice could transfer unlimited amounts
        //   - With tuple precedence, the 1000 limit is enforced regardless
        //
        // IMPLEMENTATION:
        // 1. Load request context first (lower priority)
        // 2. Apply tuple context second (overwrites any conflicting keys)
        //
        // This follows the principle: "constraints written by administrators
        // cannot be weakened by API callers"
        // ==========================================================================
        let mut context_map: HashMap<String, CelValue> = HashMap::new();

        // First, add request context (lower priority)
        context_map.extend(
            request_context
                .iter()
                .map(|(k, v)| (k.clone(), json_to_cel_value(v))),
        );

        // Then, add tuple condition context (higher priority - overwrites request context)
        if let Some(tuple_ctx) = tuple_condition_context {
            context_map.extend(
                tuple_ctx
                    .iter()
                    .map(|(k, v)| (k.clone(), json_to_cel_value(v))),
            );
        }

        cel_ctx.set_map("context", context_map);

        // Evaluate the expression with timeout to prevent DoS from expensive expressions
        let result = expr
            .evaluate_bool_with_timeout(&cel_ctx, self.config.timeout)
            .await
            .map_err(|e| DomainError::ResolverError {
                message: format!("condition evaluation failed: {}", e),
            })?;

        Ok(result)
    }
}

/// Converts a serde_json::Value to a CelValue for CEL expression evaluation.
///
/// # Type Mapping
///
/// | JSON Type | CelValue Type | Notes |
/// |-----------|---------------|-------|
/// | null | Null | |
/// | boolean | Bool | |
/// | number (fits i64) | Int | Most common integer case |
/// | number (fits u64, not i64) | UInt | Large positive integers |
/// | number (other) | Float | Floating point numbers |
/// | string | String | |
/// | array | List | Recursively converts elements |
/// | object | Map | Recursively converts values |
///
/// # Number Handling
///
/// Numbers are converted in priority order: i64 → u64 → f64.
/// This ensures large positive integers (> i64::MAX) are properly handled
/// as unsigned integers rather than being silently converted to null or
/// losing precision via floating point conversion.
///
/// # Edge Cases
///
/// - Very large numbers that don't fit in f64 will become Null (extremely rare)
/// - Nested structures are recursively converted
fn json_to_cel_value(value: &serde_json::Value) -> CelValue {
    match value {
        serde_json::Value::Null => CelValue::Null,
        serde_json::Value::Bool(b) => CelValue::Bool(*b),
        serde_json::Value::Number(n) => {
            // Try i64 first (most common case for integers)
            if let Some(i) = n.as_i64() {
                CelValue::Int(i)
            // Try u64 for large positive integers that don't fit in i64
            } else if let Some(u) = n.as_u64() {
                CelValue::UInt(u)
            // Fall back to f64 for floating point numbers
            } else if let Some(f) = n.as_f64() {
                CelValue::Float(f)
            } else {
                // This should never happen with valid JSON numbers
                CelValue::Null
            }
        }
        serde_json::Value::String(s) => CelValue::String(s.clone()),
        serde_json::Value::Array(arr) => {
            CelValue::List(arr.iter().map(json_to_cel_value).collect())
        }
        serde_json::Value::Object(obj) => {
            let map: HashMap<String, CelValue> = obj
                .iter()
                .map(|(k, v)| (k.clone(), json_to_cel_value(v)))
                .collect();
            CelValue::Map(map)
        }
    }
}
