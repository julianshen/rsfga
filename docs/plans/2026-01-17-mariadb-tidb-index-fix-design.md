# MariaDB/TiDB Index Key Length Fix

**Date**: 2026-01-17
**Status**: Approved
**Issues**: #175 (MariaDB), #176 (TiDB)

## Problem Statement

MariaDB and TiDB fail to start because database migrations fail when creating indexes. Both databases have a maximum index key length of **3072 bytes**.

### Current Index Sizes (with VARCHAR(255) × utf8mb4)

| Index | Columns | Size | Status |
|-------|---------|------|--------|
| `idx_tuples_unique` | 7 cols | 7140 bytes | Fails on TiDB |
| `idx_tuples_relation` | 4 cols | 4080 bytes | Fails on MariaDB |
| `idx_tuples_object` | 3 cols | 3060 bytes | Barely OK |
| `idx_tuples_user` | 3 cols | 3060 bytes | Barely OK |

### Root Cause

All columns use `VARCHAR(255)` with `utf8mb4` charset:
- Each column: 255 × 4 bytes = 1020 bytes
- 7 columns in unique index: 7 × 1020 = 7140 bytes (exceeds 3072)

## Solution

### Approach: Hybrid Column Size Reduction

Reduce column sizes to match OpenFGA's official MySQL schema limits while keeping our separate user column structure.

### New Column Definitions

| Column | Current | New | OpenFGA | Rationale |
|--------|---------|-----|---------|-----------|
| `store_id` | VARCHAR(255) | CHAR(26) | CHAR(26) | ULID format, fixed length |
| `object_type` | VARCHAR(255) | VARCHAR(128) | VARCHAR(128) | Matches OpenFGA |
| `object_id` | VARCHAR(255) | VARCHAR(255) | VARCHAR(255) | Keep full length |
| `relation` | VARCHAR(255) | VARCHAR(50) | VARCHAR(50) | Matches OpenFGA |
| `user_type` | VARCHAR(255) | VARCHAR(128) | (combined) | Same as object_type |
| `user_id` | VARCHAR(255) | VARCHAR(128) | (combined) | Reduced to fit index |
| `user_relation` | VARCHAR(255) | VARCHAR(50) | (combined) | Same as relation |

### New Index Sizes (utf8mb4 = 4 bytes/char)

| Index | Calculation | Total | Status |
|-------|-------------|-------|--------|
| `idx_tuples_unique` | 26×4 + 128×4 + 255×4 + 50×4 + 128×4 + 128×4 + 50×4 | 3060 bytes | OK |
| `idx_tuples_relation` | 26×4 + 128×4 + 255×4 + 50×4 | 1836 bytes | OK |
| `idx_tuples_object` | 26×4 + 128×4 + 255×4 | 1636 bytes | OK |
| `idx_tuples_user` | 26×4 + 128×4 + 128×4 | 1128 bytes | OK |

All indexes now fit under the 3072-byte limit.

## Implementation

### Migration Strategy

#### For New Installations
Create tables with new column sizes from the start.

#### For Existing Installations

1. **Pre-migration validation**: Check for data exceeding new limits
2. **Fail with clear error** if oversized data found
3. **Apply ALTER TABLE** statements to reduce column sizes

### Pre-Migration Validation Query

```sql
SELECT
    'store_id' AS field, COUNT(*) AS count
    FROM tuples WHERE CHAR_LENGTH(store_id) > 26
UNION ALL
SELECT 'object_type', COUNT(*) FROM tuples WHERE CHAR_LENGTH(object_type) > 128
UNION ALL
SELECT 'relation', COUNT(*) FROM tuples WHERE CHAR_LENGTH(relation) > 50
UNION ALL
SELECT 'user_type', COUNT(*) FROM tuples WHERE CHAR_LENGTH(user_type) > 128
UNION ALL
SELECT 'user_id', COUNT(*) FROM tuples WHERE CHAR_LENGTH(user_id) > 128
UNION ALL
SELECT 'user_relation', COUNT(*) FROM tuples WHERE CHAR_LENGTH(user_relation) > 50;
```

### Error Message (if validation fails)

```text
Migration blocked: Found data exceeding new column limits.

Affected rows:
  - user_id > 128 chars: 3 rows
  - object_type > 128 chars: 1 row

To view affected data:
  SELECT * FROM tuples WHERE CHAR_LENGTH(user_id) > 128;

Please manually update or remove oversized data before retrying migration.
```

### Migration SQL

```sql
-- Tuples table
ALTER TABLE tuples MODIFY store_id CHAR(26) NOT NULL;
ALTER TABLE tuples MODIFY object_type VARCHAR(128) NOT NULL;
ALTER TABLE tuples MODIFY relation VARCHAR(50) NOT NULL;
ALTER TABLE tuples MODIFY user_type VARCHAR(128) NOT NULL;
ALTER TABLE tuples MODIFY user_id VARCHAR(128) NOT NULL;
ALTER TABLE tuples MODIFY user_relation VARCHAR(50) DEFAULT NULL;
ALTER TABLE tuples MODIFY user_relation_key VARCHAR(50) AS (COALESCE(user_relation, '')) STORED;

-- Stores table
ALTER TABLE stores MODIFY id CHAR(26) NOT NULL;

-- Authorization models table
ALTER TABLE authorization_models MODIFY id CHAR(26) NOT NULL;
ALTER TABLE authorization_models MODIFY store_id CHAR(26) NOT NULL;
```

## Code Changes

### Files Modified

- `crates/rsfga-storage/src/mysql.rs`
  - Update CREATE TABLE statements with new column sizes
  - Add `migrate_column_sizes_if_needed()` function (detects if migration needed)
  - Add `validate_column_sizes_for_migration()` function (checks for oversized data)
  - Add `apply_column_size_migration()` function (applies ALTER TABLE statements)
- `crates/rsfga-storage/src/error.rs`
  - Add `MigrationBlocked` error variant

### Tables Affected

1. `tuples` - All columns except `object_id` and `id`
2. `stores` - `id` column only
3. `authorization_models` - `id` and `store_id` columns

## Testing

### Unit Tests

```rust
#[tokio::test]
async fn test_column_size_limits_are_enforced()

#[tokio::test]
async fn test_pre_migration_validation_detects_oversized_data()

#[tokio::test]
async fn test_migration_succeeds_with_valid_data()
```

### Integration Tests

| Database | Test |
|----------|------|
| MySQL 8.0 | Verify migration works, indexes created |
| MariaDB 10.6+ | Verify issue #175 is fixed |
| TiDB | Verify issue #176 is fixed |

### Manual Verification

- [ ] Fresh MariaDB install starts successfully *(requires manual test with MariaDB container)*
- [ ] Fresh TiDB install starts successfully *(requires manual test with TiDB container)*
- [x] Existing MySQL database migrates cleanly *(covered by `test_column_size_migration_idempotent`)*
- [x] Oversized data blocks migration with clear error *(code review - `validate_column_sizes_for_migration()`)*
- [x] All CRUD operations work after migration *(covered by `test_crud_operations_with_new_column_sizes`)*
- [x] Check/Expand/ListObjects APIs function correctly *(existing API integration tests)*

## Trade-offs

### Accepted Limitations

- `user_id` reduced from 255 to 128 characters
- `store_id` now fixed at 26 characters (ULID only)
- Existing oversized data must be manually fixed before migration

### Benefits

- Full MariaDB compatibility (fixes #175)
- Full TiDB compatibility (fixes #176)
- Matches OpenFGA's official limits
- No prefix indexes (full value matching preserved)

## References

- [OpenFGA MySQL Schema](https://github.com/openfga/openfga/tree/main/assets/migrations/mysql)
- [MariaDB Index Limitations](https://mariadb.com/kb/en/innodb-limitations/)
- Issue #175: MariaDB index key length
- Issue #176: TiDB index key length
