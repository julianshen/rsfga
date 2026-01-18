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

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::time::timeout;
use tracing::warn;

/// Timeout for cache operations (get/insert).
/// Cache should never block authorization checks; treat timeout as "cache unavailable".
const CACHE_OP_TIMEOUT: Duration = Duration::from_millis(10);

use crate::cache::CacheKey;
use crate::cel::{global_cache, CelContext, CelValue};
use crate::error::{DomainError, DomainResult};
use crate::model::{TypeConstraint, Userset};

use super::config::ResolverConfig;
use super::context::TraversalContext;
use super::traits::{ModelReader, TupleReader};
use super::types::{CheckRequest, CheckResult};

/// Metrics for cache performance monitoring.
#[derive(Debug, Default)]
pub struct CacheMetrics {
    /// Number of cache hits (result found in cache).
    pub hits: AtomicU64,
    /// Number of cache misses (result not in cache, needed graph traversal).
    pub misses: AtomicU64,
    /// Number of cache skips (caching bypassed due to contextual tuples).
    pub skips: AtomicU64,
}

impl CacheMetrics {
    /// Returns a snapshot of the current metrics.
    pub fn snapshot(&self) -> CacheMetricsSnapshot {
        CacheMetricsSnapshot {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            skips: self.skips.load(Ordering::Relaxed),
        }
    }

    /// Returns the cache hit ratio (hits / (hits + misses)).
    /// Returns 0.0 if no hits or misses have occurred.
    pub fn hit_ratio(&self) -> f64 {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }
}

/// A point-in-time snapshot of cache metrics.
#[derive(Debug, Clone, Copy)]
pub struct CacheMetricsSnapshot {
    pub hits: u64,
    pub misses: u64,
    pub skips: u64,
}

/// Type alias for boxed future to handle async recursion.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Graph resolver for permission checks.
///
/// Performs async graph traversal to determine if a user has
/// a specific permission on an object.
///
/// # Caching
///
/// When a cache is configured, the resolver will:
/// 1. Check the cache before performing expensive graph traversal
/// 2. Store successful results in the cache for future lookups
/// 3. Skip caching when contextual tuples are present (request-specific)
///
/// Cache metrics are available via `cache_metrics()` for monitoring.
pub struct GraphResolver<T, M> {
    tuple_reader: Arc<T>,
    model_reader: Arc<M>,
    config: ResolverConfig,
    /// Metrics for cache performance monitoring.
    cache_metrics: CacheMetrics,
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
            cache_metrics: CacheMetrics::default(),
        }
    }

    /// Creates a new graph resolver with custom configuration.
    pub fn with_config(tuple_reader: Arc<T>, model_reader: Arc<M>, config: ResolverConfig) -> Self {
        Self {
            tuple_reader,
            model_reader,
            config,
            cache_metrics: CacheMetrics::default(),
        }
    }

    /// Returns the cache metrics for monitoring.
    pub fn cache_metrics(&self) -> &CacheMetrics {
        &self.cache_metrics
    }

    /// Performs a permission check.
    ///
    /// # Caching Behavior
    ///
    /// When a cache is configured:
    /// 1. Checks cache before graph traversal (returns immediately on hit)
    /// 2. Stores successful results in cache after traversal
    /// 3. Skips caching when contextual tuples are present
    ///
    /// Use `cache_metrics()` to monitor cache hit/miss rates.
    pub async fn check(&self, request: &CheckRequest) -> DomainResult<CheckResult> {
        // Validate inputs
        self.validate_request(request)?;

        // Check if store exists
        if !self.tuple_reader.store_exists(&request.store_id).await? {
            return Err(DomainError::StoreNotFound {
                store_id: request.store_id.clone(),
            });
        }

        // Determine if caching should be used for this request.
        // Skip caching when:
        // - Contextual tuples are provided (request-specific, would pollute cache)
        // - Request context is non-empty (CEL conditions depend on context values,
        //   caching would return incorrect decisions for different contexts)
        let cache_and_key = if request.contextual_tuples.is_empty() && request.context.is_empty() {
            self.config.cache.as_ref().map(|cache| {
                (
                    cache,
                    CacheKey::new(
                        &request.store_id,
                        &request.object,
                        &request.relation,
                        &request.user,
                    ),
                )
            })
        } else {
            if self.config.cache.is_some() {
                // Cache is configured but skipped due to contextual tuples or context
                self.cache_metrics.skips.fetch_add(1, Ordering::Relaxed);
            }
            None
        };

        // Check cache first (if enabled and no bypass conditions)
        // Cache operations are bounded by CACHE_OP_TIMEOUT to prevent blocking auth checks.
        if let Some((cache, ref key)) = cache_and_key {
            match timeout(CACHE_OP_TIMEOUT, cache.get(key)).await {
                Ok(Some(allowed)) => {
                    // Cache hit - return immediately
                    self.cache_metrics.hits.fetch_add(1, Ordering::Relaxed);
                    return Ok(CheckResult { allowed });
                }
                Ok(None) => {
                    // Cache miss - will perform graph traversal
                    self.cache_metrics.misses.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    // Cache timeout - treat as unavailable, skip caching for this request
                    self.cache_metrics.skips.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        // Apply timeout to the entire check operation
        let ctx = TraversalContext::new();
        let check_future = self.resolve_check(request.clone(), ctx);

        let result = match timeout(self.config.timeout, check_future).await {
            Ok(result) => result,
            Err(_) => Err(DomainError::Timeout {
                duration_ms: self.config.timeout.as_millis() as u64,
            }),
        };

        // Store successful result in cache (reuse the same cache and key)
        // Ignore timeout errors - cache insert is best-effort and shouldn't block response.
        if let (Some((cache, key)), Ok(ref check_result)) = (cache_and_key, &result) {
            let _ = timeout(CACHE_OP_TIMEOUT, cache.insert(key, check_result.allowed)).await;
        }

        result
    }

    /// Expands a relation to show the tree structure of how users relate to an object.
    ///
    /// This method builds a tree representation of the relation definition,
    /// populating leaf nodes with the actual users who have the relation.
    ///
    /// # Example
    ///
    /// For a relation defined as `viewer: [user] | editor`, expanding `viewer` on
    /// `document:readme` would return a tree showing:
    /// - A union node with two branches
    /// - A leaf with direct users (from `[user]`)
    /// - A computed userset reference to `editor`
    pub async fn expand(
        &self,
        request: &super::types::ExpandRequest,
    ) -> DomainResult<super::types::ExpandResult> {
        use super::types::{ExpandResult, UsersetTree};

        // Validate request inputs
        self.validate_expand_request(request)?;

        // Check if store exists
        if !self.tuple_reader.store_exists(&request.store_id).await? {
            return Err(DomainError::StoreNotFound {
                store_id: request.store_id.clone(),
            });
        }

        // Parse object to get type
        let (object_type, object_id) = self.parse_object(&request.object)?;
        let object_type = object_type.to_string();
        let object_id = object_id.to_string();

        // Get the relation definition
        let relation_def = self
            .model_reader
            .get_relation_definition(&request.store_id, &object_type, &request.relation)
            .await?;

        // Build the expansion tree based on the userset rewrite
        let root = self
            .expand_userset(
                &request.store_id,
                &object_type,
                &object_id,
                &request.relation,
                &relation_def.rewrite,
                0, // Start at depth 0
            )
            .await?;

        Ok(ExpandResult {
            tree: UsersetTree { root },
        })
    }

    /// Recursively expands a userset rewrite into an ExpandNode tree.
    ///
    /// # Arguments
    /// * `depth` - Current recursion depth for enforcing max_depth limit (constraint C11)
    fn expand_userset<'a>(
        &'a self,
        store_id: &'a str,
        object_type: &'a str,
        object_id: &'a str,
        relation: &'a str,
        userset: &'a crate::model::Userset,
        depth: u32,
    ) -> BoxFuture<'a, DomainResult<super::types::ExpandNode>> {
        use super::types::{ExpandLeaf, ExpandLeafValue, ExpandNode};

        Box::pin(async move {
            // Check depth limit to prevent unbounded recursion (constraint C11: Fail Fast with Bounds)
            if depth >= self.config.max_depth {
                return Err(DomainError::DepthLimitExceeded {
                    max_depth: self.config.max_depth,
                });
            }

            match userset {
                crate::model::Userset::This => {
                    // Direct assignment - fetch users from tuples
                    let tuples = self
                        .tuple_reader
                        .read_tuples(store_id, object_type, object_id, relation)
                        .await?;

                    let users: Vec<String> = tuples
                        .into_iter()
                        .map(|t| {
                            if let Some(ref rel) = t.user_relation {
                                format!("{}:{}#{}", t.user_type, t.user_id, rel)
                            } else {
                                format!("{}:{}", t.user_type, t.user_id)
                            }
                        })
                        .collect();

                    Ok(ExpandNode::Leaf(ExpandLeaf {
                        name: format!("{}:{}#{}", object_type, object_id, relation),
                        value: ExpandLeafValue::Users(users),
                    }))
                }

                crate::model::Userset::ComputedUserset {
                    relation: computed_relation,
                } => {
                    // Computed userset reference
                    Ok(ExpandNode::Leaf(ExpandLeaf {
                        name: format!("{}:{}#{}", object_type, object_id, relation),
                        value: ExpandLeafValue::Computed {
                            userset: format!("{}:{}#{}", object_type, object_id, computed_relation),
                        },
                    }))
                }

                crate::model::Userset::TupleToUserset {
                    tupleset,
                    computed_userset,
                } => {
                    // Tuple-to-userset reference
                    Ok(ExpandNode::Leaf(ExpandLeaf {
                        name: format!("{}:{}#{}", object_type, object_id, relation),
                        value: ExpandLeafValue::TupleToUserset {
                            tupleset: tupleset.clone(),
                            computed_userset: computed_userset.clone(),
                        },
                    }))
                }

                crate::model::Userset::Union { children } => {
                    // Expand all children in parallel for better performance
                    let futures: Vec<_> = children
                        .iter()
                        .map(|child| {
                            self.expand_userset(
                                store_id,
                                object_type,
                                object_id,
                                relation,
                                child,
                                depth + 1,
                            )
                        })
                        .collect();
                    let nodes = futures::future::try_join_all(futures).await?;

                    Ok(ExpandNode::Union {
                        name: format!("{}:{}#{}", object_type, object_id, relation),
                        nodes,
                    })
                }

                crate::model::Userset::Intersection { children } => {
                    // Expand all children in parallel for better performance
                    let futures: Vec<_> = children
                        .iter()
                        .map(|child| {
                            self.expand_userset(
                                store_id,
                                object_type,
                                object_id,
                                relation,
                                child,
                                depth + 1,
                            )
                        })
                        .collect();
                    let nodes = futures::future::try_join_all(futures).await?;

                    Ok(ExpandNode::Intersection {
                        name: format!("{}:{}#{}", object_type, object_id, relation),
                        nodes,
                    })
                }

                crate::model::Userset::Exclusion { base, subtract } => {
                    // Expand base and subtract in parallel for better performance
                    let (base_node, subtract_node) = futures::future::try_join(
                        self.expand_userset(
                            store_id,
                            object_type,
                            object_id,
                            relation,
                            base,
                            depth + 1,
                        ),
                        self.expand_userset(
                            store_id,
                            object_type,
                            object_id,
                            relation,
                            subtract,
                            depth + 1,
                        ),
                    )
                    .await?;

                    Ok(ExpandNode::Difference {
                        name: format!("{}:{}#{}", object_type, object_id, relation),
                        base: Box::new(base_node),
                        subtract: Box::new(subtract_node),
                    })
                }
            }
        })
    }

    /// Validates the check request.
    fn validate_request(&self, request: &CheckRequest) -> DomainResult<()> {
        // Validate user format: must be in type:id format (or wildcard "*")
        if request.user.is_empty() {
            return Err(DomainError::InvalidUserFormat {
                value: request.user.clone(),
            });
        }
        if request.user != "*" && !Self::is_valid_type_id(&request.user) {
            return Err(DomainError::InvalidUserFormat {
                value: request.user.clone(),
            });
        }

        // Validate object format: must be in type:id format
        if request.object.is_empty() || !Self::is_valid_type_id(&request.object) {
            return Err(DomainError::InvalidObjectFormat {
                value: request.object.clone(),
            });
        }

        // Validate relation: must be non-empty
        if request.relation.is_empty() {
            return Err(DomainError::InvalidRelationFormat {
                value: request.relation.clone(),
            });
        }

        Ok(())
    }

    /// Validates the expand request.
    fn validate_expand_request(&self, request: &super::types::ExpandRequest) -> DomainResult<()> {
        // Validate object format: must be in type:id format
        if request.object.is_empty() || !Self::is_valid_type_id(&request.object) {
            return Err(DomainError::InvalidObjectFormat {
                value: request.object.clone(),
            });
        }

        // Validate relation: must be non-empty
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

    /// Checks if a string is in valid `type:id` format.
    ///
    /// Returns `true` if the value contains a colon with non-empty parts on both sides.
    /// This is a common validation pattern used throughout the authorization model.
    #[inline]
    fn is_valid_type_id(value: &str) -> bool {
        match value.split_once(':') {
            Some((type_part, id_part)) => !type_part.is_empty() && !id_part.is_empty(),
            None => false,
        }
    }

    /// Parses an object string into type and id.
    ///
    /// Validates that the object is in `type:id` format where both
    /// type and id are non-empty strings.
    fn parse_object<'a>(&self, object: &'a str) -> DomainResult<(&'a str, &'a str)> {
        match object.split_once(':') {
            Some((type_part, id_part)) if !type_part.is_empty() && !id_part.is_empty() => {
                Ok((type_part, id_part))
            }
            _ => Err(DomainError::InvalidObjectFormat {
                value: object.to_string(),
            }),
        }
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
                    message: format!("condition not found: {condition_name}"),
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
        // Build the "request" map from request context + tuple condition context.
        // OpenFGA CEL expressions access values via "request." prefix (e.g., request.current_time).
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

        // Note: OpenFGA expressions use "request" prefix (e.g., request.current_time)
        cel_ctx.set_map("request", context_map);

        // Evaluate the expression with timeout to prevent DoS from expensive expressions
        let result = expr
            .evaluate_bool_with_timeout(&cel_ctx, self.config.timeout)
            .await
            .map_err(|e| DomainError::ResolverError {
                message: format!("condition evaluation failed: {e}"),
            })?;

        Ok(result)
    }

    /// Lists objects of a given type that a user has a specific relation to.
    ///
    /// # DoS Protection (Constraint C11)
    ///
    /// This method applies a hard limit of `max_candidates` on the number of
    /// candidate objects to check. This prevents memory exhaustion attacks
    /// where an attacker could enumerate a large number of objects.
    ///
    /// The default limit is 1,000 objects, matching OpenFGA's default behavior.
    ///
    /// # Algorithm
    ///
    /// 1. Query all objects of the requested type (up to max_candidates limit)
    /// 2. For each candidate object, run a parallel permission check
    /// 3. Return objects where the check returns true
    ///
    /// # Example
    ///
    /// ```ignore
    /// let request = ListObjectsRequest::new(
    ///     "store-id",
    ///     "user:alice",
    ///     "viewer",
    ///     "document",
    /// );
    /// let result = resolver.list_objects(&request, 1000).await?;
    /// // result.objects = ["document:readme", "document:design"]
    /// ```
    pub async fn list_objects(
        &self,
        request: &super::types::ListObjectsRequest,
        max_candidates: usize,
    ) -> DomainResult<super::types::ListObjectsResult> {
        // Check if store exists
        if !self.tuple_reader.store_exists(&request.store_id).await? {
            return Err(DomainError::StoreNotFound {
                store_id: request.store_id.clone(),
            });
        }

        // Validate that the user is not a wildcard (not allowed for ListObjects)
        if request.user.contains('*') {
            return Err(DomainError::InvalidUserFormat {
                value: request.user.clone(),
            });
        }

        // Validate user format
        if !Self::is_valid_type_id(&request.user) {
            return Err(DomainError::InvalidUserFormat {
                value: request.user.clone(),
            });
        }

        // Validate relation format
        if request.relation.is_empty() {
            return Err(DomainError::InvalidRelationFormat {
                value: request.relation.clone(),
            });
        }

        // Validate object type format (must not be empty, must not contain colon)
        if request.object_type.is_empty() || request.object_type.contains(':') {
            return Err(DomainError::InvalidObjectFormat {
                value: request.object_type.clone(),
            });
        }

        // Get all objects of the requested type (limited to max_candidates + 1 for truncation detection)
        // This is the DoS protection: we bound the number of candidates BEFORE
        // running expensive permission checks.
        let limit = max_candidates.saturating_add(1);
        let mut candidates = self
            .tuple_reader
            .get_objects_of_type(&request.store_id, &request.object_type, limit)
            .await?;

        // Check if we hit the limit (truncation detection)
        let truncated = if candidates.len() > max_candidates {
            candidates.pop(); // Remove the extra candidate
            true
        } else {
            false
        };

        // Early exit: skip expensive permission checks if no candidates found
        if candidates.is_empty() {
            return Ok(super::ListObjectsResult {
                objects: Vec::new(),
                truncated: false,
            });
        }

        // Run parallel permission checks on candidates with concurrency limit
        // Constraint C11: Fail Fast with Bounds - we limit concurrency to avoid
        // exhausting resources even effectively "bounded" tasks.
        const MAX_CONCURRENT_CHECKS: usize = 50;
        // Per-operation timeout for individual permission checks.
        // Prevents a single slow check from blocking the entire operation.
        const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

        // Track if any errors occurred during permission checks
        let had_errors = std::sync::atomic::AtomicBool::new(false);

        // Clone shared data once outside the loop to avoid per-candidate cloning
        let shared_contextual_tuples = request.contextual_tuples.clone();
        let shared_context = request.context.clone();

        let objects = futures::stream::iter(candidates)
            .map(|object_id| {
                let check_request = CheckRequest::with_context(
                    request.store_id.clone(),
                    request.user.clone(),
                    request.relation.clone(),
                    format!("{}:{}", request.object_type, object_id),
                    shared_contextual_tuples.as_ref().clone(),
                    shared_context.as_ref().clone(),
                );
                async move {
                    // Apply per-operation timeout to prevent indefinite blocking
                    let result = timeout(CHECK_TIMEOUT, self.check(&check_request)).await;
                    (object_id, result)
                }
            })
            .buffer_unordered(MAX_CONCURRENT_CHECKS)
            .filter_map(|(object_id, result)| {
                let had_errors = &had_errors;
                async move {
                    match result {
                        Ok(Ok(CheckResult { allowed: true })) => {
                            Some(format!("{}:{}", request.object_type, object_id))
                        }
                        Ok(Ok(CheckResult { allowed: false })) => None,
                        Ok(Err(e)) => {
                            warn!(
                                store_id = %request.store_id,
                                object_type = %request.object_type,
                                object_id = %object_id,
                                error = %e,
                                "Permission check failed during list_objects"
                            );
                            had_errors.store(true, std::sync::atomic::Ordering::Relaxed);
                            None
                        }
                        Err(_timeout) => {
                            warn!(
                                store_id = %request.store_id,
                                object_type = %request.object_type,
                                object_id = %object_id,
                                "Permission check timed out during list_objects"
                            );
                            had_errors.store(true, std::sync::atomic::Ordering::Relaxed);
                            None
                        }
                    }
                }
            })
            .collect::<Vec<_>>()
            .await;

        // Truncated if we hit the limit OR if any errors occurred (incomplete results)
        let result_truncated = truncated || had_errors.load(std::sync::atomic::Ordering::Relaxed);

        Ok(super::types::ListObjectsResult {
            objects,
            truncated: result_truncated,
        })
    }

    /// Lists users that have a specific relation to an object.
    ///
    /// This is the inverse of `list_objects` - given an object and relation,
    /// it returns all users that have that relation to the object.
    ///
    /// # Arguments
    ///
    /// * `request` - The ListUsers request containing object, relation, and user filters
    /// * `max_results` - Maximum number of results to return (for DoS protection)
    ///
    /// # DoS Protection
    ///
    /// This method applies the same protections as `list_objects`:
    /// - Result limit with truncation flag
    /// - Per-operation timeout (30s)
    /// - User filter validation
    ///
    /// # Example
    ///
    /// ```ignore
    /// let request = ListUsersRequest::new(
    ///     "store-id",
    ///     "document:readme",
    ///     "viewer",
    ///     vec![UserFilter::new("user")],
    /// );
    /// let result = resolver.list_users(&request, 1000).await?;
    /// // result.users = [UserResult::Object { user_type: "user", user_id: "alice" }, ...]
    /// // result.truncated = false (or true if more than 1000 users exist)
    /// ```
    pub async fn list_users(
        &self,
        request: &super::types::ListUsersRequest,
        max_results: usize,
    ) -> DomainResult<super::types::ListUsersResult> {
        // Per-operation timeout for the entire list_users operation.
        // Prevents indefinite blocking on pathological graphs.
        const LIST_USERS_TIMEOUT: Duration = Duration::from_secs(30);

        // Wrap the entire operation with a timeout
        match timeout(
            LIST_USERS_TIMEOUT,
            self.list_users_inner(request, max_results),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(DomainError::OperationTimeout {
                operation: "list_users".to_string(),
                timeout_secs: LIST_USERS_TIMEOUT.as_secs(),
            }),
        }
    }

    /// Inner implementation of list_users without timeout wrapper.
    async fn list_users_inner(
        &self,
        request: &super::types::ListUsersRequest,
        max_results: usize,
    ) -> DomainResult<super::types::ListUsersResult> {
        use super::types::{ListUsersResult, UserResult};
        use std::collections::HashSet;

        // Check if store exists
        if !self.tuple_reader.store_exists(&request.store_id).await? {
            return Err(DomainError::StoreNotFound {
                store_id: request.store_id.clone(),
            });
        }

        // Parse object into type and id
        let (object_type, object_id) = self.parse_object(&request.object)?;

        // Validate relation format
        if request.relation.is_empty() {
            return Err(DomainError::InvalidRelationFormat {
                value: request.relation.clone(),
            });
        }

        // Validate user_filters is not empty (OpenFGA requirement)
        if request.user_filters.is_empty() {
            return Err(DomainError::ResolverError {
                message: "user_filters cannot be empty".to_string(),
            });
        }

        // Validate user_filter format: type_name must be non-empty and contain valid characters
        for filter in &request.user_filters {
            if filter.type_name.is_empty() {
                return Err(DomainError::ResolverError {
                    message: "user_filter type cannot be empty".to_string(),
                });
            }
            // Only allow alphanumeric, underscore, and dash (same as object type validation)
            if !filter
                .type_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(DomainError::ResolverError {
                    message: format!(
                        "user_filter type contains invalid characters: {}",
                        filter.type_name
                    ),
                });
            }
            // Also validate relation if provided
            if let Some(ref relation) = filter.relation {
                if relation.is_empty() {
                    return Err(DomainError::ResolverError {
                        message: "user_filter relation cannot be empty when provided".to_string(),
                    });
                }
                if !relation
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    return Err(DomainError::ResolverError {
                        message: format!(
                            "user_filter relation contains invalid characters: {}",
                            relation
                        ),
                    });
                }
            }
        }

        // Collect users from all sources using a HashSet to deduplicate.
        // We collect all matching users first, then apply truncation at the end.
        // This ensures consistent deduplication before applying the limit.
        let mut users: HashSet<UserResult> = HashSet::new();

        // 1. Get tuples directly assigned to this object/relation
        let tuples = self
            .tuple_reader
            .read_tuples(&request.store_id, object_type, object_id, &request.relation)
            .await?;

        for tuple in tuples {
            // Check if this tuple's user matches any user filter
            if self.user_matches_filters(
                &tuple.user_type,
                tuple.user_relation.as_deref(),
                &request.user_filters,
            ) {
                // Check for wildcard
                if tuple.user_id == "*" {
                    users.insert(UserResult::wildcard(&tuple.user_type));
                } else if let Some(ref rel) = tuple.user_relation {
                    // Userset reference (e.g., group:eng#member)
                    users.insert(UserResult::userset(&tuple.user_type, &tuple.user_id, rel));
                } else {
                    // Direct user
                    users.insert(UserResult::object(&tuple.user_type, &tuple.user_id));
                }
            }
        }

        // 2. Also check contextual tuples
        for tuple in request.contextual_tuples.iter() {
            // Parse the contextual tuple's object
            if let Ok((ctx_obj_type, ctx_obj_id)) = self.parse_object(&tuple.object) {
                // Check if this tuple applies to our target object
                if ctx_obj_type == object_type
                    && ctx_obj_id == object_id
                    && tuple.relation == request.relation
                {
                    // Parse the user from the contextual tuple
                    match self.parse_user_string(&tuple.user) {
                        Ok((user_type, user_id, user_relation)) => {
                            if self.user_matches_filters(
                                &user_type,
                                user_relation.as_deref(),
                                &request.user_filters,
                            ) {
                                if user_id == "*" {
                                    users.insert(UserResult::wildcard(user_type));
                                } else if let Some(rel) = user_relation {
                                    users.insert(UserResult::userset(user_type, user_id, rel));
                                } else {
                                    users.insert(UserResult::object(user_type, user_id));
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                store_id = %request.store_id,
                                user = %tuple.user,
                                error = %e,
                                "Invalid contextual tuple user format in ListUsers, skipping"
                            );
                        }
                    }
                }
            }
        }

        // 3. Handle computed relations (union, intersection, etc.)
        // Get the authorization model to check for computed relations
        let model = self.model_reader.get_model(&request.store_id).await?;

        // Find the type definition for the object type
        if let Some(type_def) = model
            .type_definitions
            .iter()
            .find(|td| td.type_name == object_type)
        {
            // Find the relation definition
            if let Some(relation_def) = type_def
                .relations
                .iter()
                .find(|r| r.name == request.relation)
            {
                // Recursively collect users from computed usersets
                self.collect_users_from_userset(
                    &request.store_id,
                    object_type,
                    object_id,
                    &relation_def.rewrite,
                    &request.user_filters,
                    &request.contextual_tuples,
                    &mut users,
                    0,
                )
                .await?;
            }
        }

        // Check if we exceeded the effective limit (max_results + 1)
        // If so, we need to truncate and set the truncated flag
        let mut user_vec: Vec<_> = users.into_iter().collect();
        let truncated = user_vec.len() > max_results;
        if truncated {
            let original_count = user_vec.len();
            user_vec.truncate(max_results);
            warn!(
                store_id = %request.store_id,
                object = %request.object,
                relation = %request.relation,
                max_results = %max_results,
                total_users = %original_count,
                "ListUsers results truncated due to max_results limit"
            );
        }

        Ok(ListUsersResult {
            users: user_vec,
            // Note: excluded_users is currently always empty.
            // Full exclusion support (tracking which users are excluded by 'but not' relations)
            // would require traversing the exclusion branch and comparing with base results.
            // This matches OpenFGA's current behavior for ListUsers API.
            excluded_users: Vec::new(),
            truncated,
        })
    }

    /// Helper to parse a user string like "user:alice" or "group:eng#member" into components.
    /// Returns DomainResult for consistent error handling.
    fn parse_user_string(&self, user: &str) -> DomainResult<(String, String, Option<String>)> {
        let (user_type, user_id, user_relation) =
            if let Some((obj_part, relation)) = user.split_once('#') {
                // Userset format: "type:id#relation"
                if relation.is_empty() {
                    return Err(DomainError::InvalidUserFormat {
                        value: user.to_string(),
                    });
                }
                let (user_type, user_id) =
                    obj_part
                        .split_once(':')
                        .ok_or_else(|| DomainError::InvalidUserFormat {
                            value: user.to_string(),
                        })?;
                (user_type, user_id, Some(relation))
            } else {
                // Direct format: "type:id"
                let (user_type, user_id) =
                    user.split_once(':')
                        .ok_or_else(|| DomainError::InvalidUserFormat {
                            value: user.to_string(),
                        })?;
                (user_type, user_id, None)
            };

        // Validate non-empty parts
        if user_type.is_empty() || user_id.is_empty() {
            return Err(DomainError::InvalidUserFormat {
                value: user.to_string(),
            });
        }

        Ok((
            user_type.to_string(),
            user_id.to_string(),
            user_relation.map(String::from),
        ))
    }

    /// Checks if a user type (and optional relation) matches any of the user filters.
    fn user_matches_filters(
        &self,
        user_type: &str,
        user_relation: Option<&str>,
        user_filters: &[super::types::UserFilter],
    ) -> bool {
        for filter in user_filters {
            if filter.type_name == user_type {
                match (&filter.relation, user_relation) {
                    // Filter requires a relation and it matches
                    (Some(filter_rel), Some(user_rel)) if filter_rel == user_rel => return true,
                    // Filter has no relation requirement and user has no relation
                    (None, None) => return true,
                    // Filter has no relation requirement - match any user of this type
                    // (including wildcards and usersets)
                    (None, _) => return true,
                    _ => continue,
                }
            }
        }
        false
    }

    /// Recursively collects users from a userset rewrite (handles union, intersection, etc.)
    ///
    /// # Clippy Allows
    ///
    /// - `too_many_arguments`: 8 arguments are required to track recursive traversal state
    ///   (store_id, object info, userset, filters, contextual tuples, results set, depth).
    ///   Grouping these into a context struct would add allocation overhead per recursion.
    ///
    /// - `only_used_in_recursion`: The `depth` parameter is intentionally only used
    ///   in recursive calls to track traversal depth and enforce `max_depth` limit.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::only_used_in_recursion)]
    fn collect_users_from_userset<'a>(
        &'a self,
        store_id: &'a str,
        object_type: &'a str,
        object_id: &'a str,
        userset: &'a Userset,
        user_filters: &'a [super::types::UserFilter],
        contextual_tuples: &'a Arc<Vec<super::types::ContextualTuple>>,
        users: &'a mut HashSet<super::types::UserResult>,
        depth: u32,
    ) -> BoxFuture<'a, DomainResult<()>> {
        Box::pin(async move {
            // Depth limit to prevent infinite recursion - return error to indicate incomplete results
            if depth >= self.config.max_depth {
                return Err(DomainError::DepthLimitExceeded {
                    max_depth: self.config.max_depth,
                });
            }

            match userset {
                Userset::This => {
                    // Already handled in main list_users - direct tuples
                }
                Userset::ComputedUserset { relation } => {
                    // Get users from the computed relation
                    let tuples = self
                        .tuple_reader
                        .read_tuples(store_id, object_type, object_id, relation)
                        .await?;

                    for tuple in tuples {
                        if self.user_matches_filters(
                            &tuple.user_type,
                            tuple.user_relation.as_deref(),
                            user_filters,
                        ) {
                            if tuple.user_id == "*" {
                                users.insert(super::types::UserResult::wildcard(&tuple.user_type));
                            } else if let Some(ref rel) = tuple.user_relation {
                                users.insert(super::types::UserResult::userset(
                                    &tuple.user_type,
                                    &tuple.user_id,
                                    rel,
                                ));
                            } else {
                                users.insert(super::types::UserResult::object(
                                    &tuple.user_type,
                                    &tuple.user_id,
                                ));
                            }
                        }
                    }

                    // Also check contextual tuples for this relation
                    for ctx_tuple in contextual_tuples.iter() {
                        if let Ok((ctx_obj_type, ctx_obj_id)) = self.parse_object(&ctx_tuple.object)
                        {
                            if ctx_obj_type == object_type
                                && ctx_obj_id == object_id
                                && ctx_tuple.relation == *relation
                            {
                                if let Ok((user_type, user_id, user_relation)) =
                                    self.parse_user_string(&ctx_tuple.user)
                                {
                                    if self.user_matches_filters(
                                        &user_type,
                                        user_relation.as_deref(),
                                        user_filters,
                                    ) {
                                        if user_id == "*" {
                                            users.insert(super::types::UserResult::wildcard(
                                                user_type,
                                            ));
                                        } else if let Some(rel) = user_relation {
                                            users.insert(super::types::UserResult::userset(
                                                user_type, user_id, rel,
                                            ));
                                        } else {
                                            users.insert(super::types::UserResult::object(
                                                user_type, user_id,
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Userset::TupleToUserset {
                    tupleset,
                    computed_userset,
                } => {
                    // Get tuples from the tupleset relation (e.g., "parent")
                    let tupleset_tuples = self
                        .tuple_reader
                        .read_tuples(store_id, object_type, object_id, tupleset)
                        .await?;

                    // For each parent, recursively get users with the computed_userset relation
                    for tuple in tupleset_tuples {
                        // Use tuple fields directly instead of parsing
                        let parent_type = &tuple.user_type;
                        let parent_id = &tuple.user_id;

                        // Read users from the parent's computed_userset relation
                        let parent_tuples = self
                            .tuple_reader
                            .read_tuples(store_id, parent_type, parent_id, computed_userset)
                            .await?;

                        for parent_tuple in parent_tuples {
                            if self.user_matches_filters(
                                &parent_tuple.user_type,
                                parent_tuple.user_relation.as_deref(),
                                user_filters,
                            ) {
                                if parent_tuple.user_id == "*" {
                                    users.insert(super::types::UserResult::wildcard(
                                        &parent_tuple.user_type,
                                    ));
                                } else if let Some(ref rel) = parent_tuple.user_relation {
                                    users.insert(super::types::UserResult::userset(
                                        &parent_tuple.user_type,
                                        &parent_tuple.user_id,
                                        rel,
                                    ));
                                } else {
                                    users.insert(super::types::UserResult::object(
                                        &parent_tuple.user_type,
                                        &parent_tuple.user_id,
                                    ));
                                }
                            }
                        }
                    }
                }
                Userset::Union { children } => {
                    // Collect users from all children
                    for child in children {
                        self.collect_users_from_userset(
                            store_id,
                            object_type,
                            object_id,
                            child,
                            user_filters,
                            contextual_tuples,
                            users,
                            depth + 1,
                        )
                        .await?;
                    }
                }
                Userset::Intersection { children } => {
                    // For intersection, we need to find users that are in ALL children
                    // Start with users from the first child, then intersect with others
                    if let Some((first, rest)) = children.split_first() {
                        let mut intersection_users: HashSet<super::types::UserResult> =
                            HashSet::new();
                        self.collect_users_from_userset(
                            store_id,
                            object_type,
                            object_id,
                            first,
                            user_filters,
                            contextual_tuples,
                            &mut intersection_users,
                            depth + 1,
                        )
                        .await?;

                        for child in rest {
                            let mut child_users: HashSet<super::types::UserResult> = HashSet::new();
                            self.collect_users_from_userset(
                                store_id,
                                object_type,
                                object_id,
                                child,
                                user_filters,
                                contextual_tuples,
                                &mut child_users,
                                depth + 1,
                            )
                            .await?;

                            // Keep only users that are in both sets
                            intersection_users.retain(|u| child_users.contains(u));
                        }

                        users.extend(intersection_users);
                    }
                }
                Userset::Exclusion { base, subtract } => {
                    // Get users from base, then remove those in subtract
                    let mut base_users: HashSet<super::types::UserResult> = HashSet::new();
                    self.collect_users_from_userset(
                        store_id,
                        object_type,
                        object_id,
                        base,
                        user_filters,
                        contextual_tuples,
                        &mut base_users,
                        depth + 1,
                    )
                    .await?;

                    let mut subtract_users: HashSet<super::types::UserResult> = HashSet::new();
                    self.collect_users_from_userset(
                        store_id,
                        object_type,
                        object_id,
                        subtract,
                        user_filters,
                        contextual_tuples,
                        &mut subtract_users,
                        depth + 1,
                    )
                    .await?;

                    // Add base users that are not in subtract
                    for user in base_users {
                        if !subtract_users.contains(&user) {
                            users.insert(user);
                        }
                    }
                }
            }

            Ok(())
        })
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
