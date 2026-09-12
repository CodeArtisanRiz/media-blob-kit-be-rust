# MediaBlobKit Backend — Comprehensive Analysis Report

Generated: 2026-09-12

---

## 1. BUGS (Logic Errors, Missing Error Handling, Race Conditions, Edge Cases)

### 1.1 CRITICAL BUGS

#### BUG-1: SQL Injection in Raw SQL Statements
**Files**: `src/routes/upload.rs:111-114`, `src/routes/upload.rs:262-266`, `src/routes/files.rs:362-366`, `src/services/worker.rs:347-351`

All quota-tracking SQL uses string interpolation with `format!()`:
```rust
format!("UPDATE projects SET storage_used_bytes = storage_used_bytes + {} WHERE id = '{}'", size, project.id)
```
While `size` is an i64 and `project.id` is a UUID (both non-user-controlled), this pattern is dangerous: if any of these values ever came from user input, it's SQL injection. The `project.id` comes from a trusted middleware lookup so it's **safe in practice**, but the pattern should use parameterized queries for defense-in-depth.

#### BUG-2: API Key Query Parameter Parsing Logic Error
**File**: `src/middleware/api_key.rs:48-56`

```rust
query_str.split('&').find_map(|pair| {
    let mut parts = pair.split('=');
    if parts.next() == Some("api_key") || parts.next() == Some("key") {
        parts.next().map(|v| v.to_string())
    } else {
        None
    }
})
```
The `||` short-circuit means: if the first `parts.next()` returns `"api_key"`, the condition is true but `parts.next()` inside the block consumes the **value** correctly. BUT if the first `parts.next()` is NOT `"api_key"`, then `parts.next()` is called again for the `"key"` check — this **consumes the value**, so the subsequent `parts.next()` inside the `if` block returns `None`. The `?key=VALUE` parameter will **never work** — it always returns `None`.

Additionally, `?api_key=VALUE` works by accident because after `parts.next() == Some("api_key")` is true, the next `parts.next()` correctly gets the value.

#### BUG-3: Refresh Token Not Rotated on Refresh
**File**: `src/routes/auth.rs:225-252`

The `POST /auth/refresh` endpoint revokes the old refresh token but **does not issue a new one**. It only returns a new `access_token`. After one refresh, the user has no valid refresh token and must re-login when the new access token expires. This breaks the expected refresh token rotation pattern.

#### BUG-4: `ensure_bucket_exists()` Called on Every Upload
**File**: `src/routes/upload.rs:89,198`

Every file/image upload calls `ensure_bucket_exists()` which does a `HEAD` bucket + `PUT` bucket policy on every request. This is:
- A performance penalty (2 extra S3 API calls per upload)
- A potential failure path (if policy-setting fails, the upload is rejected even though the bucket exists)

#### BUG-5: S3 Public Policy Set on Every `ensure_bucket_exists` Call
**File**: `src/services/s3.rs:93-94`

Even when the bucket already exists (the `Ok(_)` branch), `set_public_policy()` is called. This is an unnecessary write operation on every upload that could fail and block uploads.

#### BUG-6: Storage Quota Silently Ignored — No Enforcement
**Files**: `src/routes/upload.rs:111-115`, `src/routes/upload.rs:262-266`

The code updates `storage_used_bytes` after upload but **never checks** if the upload would exceed `storage_limit_bytes`. Quota tracking exists but no enforcement. Same for `transforms_used` vs `transforms_limit` — the worker increments `transforms_used` but never checks the limit before processing.

#### BUG-7: Variant Storage Size Not Tracked
**File**: `src/services/worker.rs:330`

When variants are generated and uploaded to S3, their sizes are never added to `storage_used_bytes`. Only the original file size is tracked. This means actual S3 usage can far exceed the tracked storage.

### 1.2 MODERATE BUGS

#### BUG-8: `file.size` Cast Truncation
**File**: `src/routes/upload.rs:81`

```rust
let size = data.len() as i64;
```
On 32-bit platforms, `data.len()` returns `usize` (32-bit), so files >2GB would overflow. On 64-bit platforms this is fine, but the body limit is 50MB so it's practically safe.

#### BUG-9: Race Condition in Worker Stuck-Job Recovery
**File**: `src/services/worker.rs:80-91`

`recover_stuck_jobs()` resets ALL `processing` → `pending` unconditionally on startup. The comment acknowledges this is unsafe in multi-worker environments. In a single-worker setup, this is correct, but if `WORKER_CONCURRENCY > 1`, a job could be legitimately processing in another tokio task when the worker restarts (e.g., during hot reload). However, since the worker runs in-process, a restart kills all tasks, so this is safe in practice.

#### BUG-10: Race Condition in Rate Limiter — Mutex Contention
**File**: `src/middleware/rate_limit.rs:29-54`

The rate limiter uses `tokio::sync::Mutex<HashMap>`, which means every request globally acquires a single lock. Under high concurrency, this becomes a bottleneck. The cleanup threshold (`> 10_000`) also means stale entries accumulate until that threshold.

#### BUG-11: No Validation of `page=0` in Pagination
**Files**: `src/routes/files.rs:81,139`, `src/routes/projects.rs:136,146`

If `page=0` is sent, `paginator.fetch_page(page - 1)` becomes `fetch_page(u64::MAX)` due to unsigned underflow. SeaORM will attempt to fetch an impossibly high offset, returning empty results (not a crash, but confusing behavior).

#### BUG-12: `update_user` Endpoint Missing from OpenAPI Docs
**File**: `src/routes/mod.rs`

The route `/users/{id}` PATCH is registered (line 175), and the handler has utoipa annotations, but `users::update_user` is **not listed** in the `#[openapi(paths(...))]` macro. It won't appear in Swagger UI.

#### BUG-13: Admin Jobs Endpoint Has No Role Gate at Route Level
**File**: `src/routes/mod.rs:164`

`/admin/jobs` is in `protected_routes` (auth required) but not in `su_routes`. The handler does manual role-checking (line 153 in jobs.rs), but returning `Err(AppError::Unauthorized(...))` for `Role::User` when it should arguably be `AppError::Forbidden(...)`. Also, the SSE endpoint `/admin/jobs/events` has **zero role checking** — any authenticated user can subscribe.

#### BUG-14: `list_admin_jobs` Fetches ALL Jobs Then Paginates In-Memory
**File**: `src/routes/jobs.rs:174`

The query fetches ALL matching jobs from the database, then slices in Rust for pagination. For projects with thousands of jobs, this loads everything into memory. Should use database-level pagination.

#### BUG-15: Cleanup Scheduler Runs Immediately on Startup
**File**: `src/services/cleanup.rs:18,24`

`tokio::time::interval()` ticks immediately on the first `.tick().await`. This means on every server start, the cleanup runs immediately. For a daily task, this is usually undesirable and could be slow if there are many soft-deleted projects.

#### BUG-16: File Deletion Doesn't Delete Associated Jobs
**File**: `src/routes/files.rs:350-354`

Files are deleted from DB but the associated jobs are only cleaned up by CASCADE constraint (if the FK has `ON DELETE CASCADE`). The jobs migration does have CASCADE, so this works, but any in-flight jobs will have their DB record removed while the worker might still be processing, leading to a DB update error in the worker's `perform_job()`.

### 1.3 MINOR BUGS / EDGE CASES

#### BUG-17: `sanitize_bucket_name` Applied to Project Name in S3 Keys
**File**: `src/routes/upload.rs:48-52`

The sanitizer replaces all non-alphanumeric chars with `-`. Two different project names like "My Project" and "My-Project" produce the same sanitized prefix `my-project`, potentially leading to S3 key collisions across projects. The UUID suffix prevents true collisions, but it could lead to confusing S3 prefixes.

#### BUG-18: `upload_file` Returns 200, Not 201
**File**: `src/routes/upload.rs:121`

Standard REST practice is to return `201 Created` for resource creation. The OpenAPI spec says `status = 200`.

#### BUG-19: Variant URL Extraction in `delete_file` Duplicates Logic
**File**: `src/routes/files.rs:329-339`

The S3 key extraction from variant URLs is done with inline logic that partially duplicates `utils::extract_s3_key()`. This inline version handles the bucket-in-path case differently.

#### BUG-20: `reset_db.rs` Drops "user" but Tables Are Named "users"
**File**: `utils/reset_db.rs:15`

`DROP TABLE IF EXISTS "user" CASCADE` — but the actual table is named `users`. This line is a no-op and doesn't clean up the users table. Line 39 does correctly drop `users`.

---

## 2. MISSING FEATURES / INCOMPLETE IMPLEMENTATIONS

### 2.1 Phase 13 (Logs) — Entirely Unimplemented
Per IMPLEMENTATION.md, the entire logging/sync system (logs table, sync job, cleanup) is not built. The config references `LOG_SYNC_URL` etc. in the doc but these aren't in `Config`.

### 2.2 No "Keep Original" Logic
There is no explicit "keep_original" variant concept. When an image is uploaded:
- The original is always stored at `{prefix}/images/original/{uuid}.{ext}`
- Variants are generated based on project settings
- The `variants_json` field only stores variant URLs, not the original
- The original URL is returned as `original_url` in the upload response
- The file's `s3_key` field stores the original's S3 key

**Missing**: There is no way for a project to configure whether the original should be kept after variant generation. The original is always kept. If a user only wants processed variants (e.g., to save storage), there's no option to delete the original after processing.

### 2.3 No `keep_original` as a Variant
The system doesn't support a "variant" called `original` that maps to the unchanged file. Variants always involve processing. If the frontend wants to display the original alongside variants in the same data structure, it must separately use `original_url` from the upload response or `url` from the file response.

### 2.4 No File Update / Re-upload
No endpoint to replace a file's content while keeping the same ID/URL.

### 2.5 No Batch Upload
Only single-file upload per request is supported.

### 2.6 No Webhook Notifications
No callback URL mechanism to notify external services when processing completes.

### 2.7 No Image Metadata Extraction
Image dimensions, color space, EXIF data, etc. are not extracted or stored.

### 2.8 No CDN / Custom Domain Support
URLs are always the raw S3/MinIO endpoint URL. No support for CDN prefix or custom domain configuration.

### 2.9 No Health Check Endpoint
No `/health` or `/ready` endpoint for container orchestration.

### 2.10 Orphaned S3 Object Scan Not Implemented
IMPLEMENTATION.md mentions this as TODO under Phase 10.

### 2.11 No Transform Quota Reset Mechanism
`transforms_used` is tracked but never reset (e.g., monthly). There's no cron/scheduler to reset it.

### 2.12 No Description Clearing on Project Update
In `update_project`, if `description` is `Some("")`, it sets `Some("")` — there's no way to clear it to `None`.

---

## 3. PROJECT SETTINGS / CONFIG MODEL & VARIANT HANDLING

### 3.1 Config Model

**Global config** (`src/config.rs`): Environment-variable based, loaded once via `OnceLock`. Fields:
- Server: `host`, `port`
- Database: `database_url`
- Auth: `jwt_secret`
- S3: `aws_region`, `aws_access_key_id`, `aws_secret_access_key`, `s3_bucket_name`, `s3_endpoint`
- Worker: `worker_concurrency` (default 1)
- Startup: `auto_migrate` (default true), `su_username`, `su_password`

### 3.2 Project Settings Model (`src/models/settings.rs`)

```rust
pub struct ProjectSettings {
    pub variants: Option<HashMap<String, VariantConfig>>,
}

pub struct VariantConfig {
    pub format: Option<String>,    // avif, webp, png, jpg, jpeg, original
    pub quality: Option<u8>,       // 0-255 (default 80)
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub fit: Option<String>,       // cover, contain, inside, fill
}
```

Stored as JSON in `projects.settings` column. Example:
```json
{
  "variants": {
    "thumbnail": { "width": 200, "height": 200, "format": "webp", "quality": 80, "fit": "cover" },
    "card": { "width": 600, "format": "webp", "quality": 85 }
  }
}
```

### 3.3 How Variants Are Handled

1. **On image upload** (`upload_image`):
   - Project settings are read from `ProjectContext.settings` (parsed in API key middleware)
   - For each variant in settings, a future URL is computed and stored in `variants_json`
   - A job is created with the variant config as payload
   - SVGs and images with no variants configured skip processing (marked `ready` immediately)

2. **On worker processing** (`process_image_logic`):
   - Original image is downloaded from S3
   - Each variant is processed (resize + format conversion) using the `image` crate
   - Processed variants are uploaded to S3
   - `variants_json` is **overwritten** with the new variant URLs (not merged)
   - File status is set to `ready`

3. **On sync-variants** (`POST /projects/{id}/sync-variants`):
   - Creates a job per image file in the project
   - Uses current project settings (not the settings at upload time)
   - This means changing settings and syncing regenerates all variants

### 3.4 Variant Handling Issues

- **No validation** of variant config values (quality can be >100, negative widths, unknown fit modes, etc.)
- **No limit** on number of variants per project
- **Overwrite semantics**: When variants are regenerated, old S3 objects at different paths (e.g., different format extension) are NOT deleted. Only `variants_json` is overwritten, leaving orphaned S3 objects.
- **Quality field**: `u8` allows 0-255, but JPEG quality is 0-100. Values >100 would be passed to the encoder (behavior depends on the `image` crate).

---

## 4. "KEEP ORIGINAL" LOGIC

### Current Behavior:
- **The original file is ALWAYS kept.** There is no "keep_original" toggle.
- Original images are stored at `{prefix}/images/original/{uuid}.{ext}`
- The original S3 key is persisted in `files.s3_key`
- The original URL is returned as `original_url` in `ImageUploadResponse`
- When variants are generated, the original is downloaded, processed, and variants are uploaded; the original is never deleted

### What Doesn't Exist:
- No `keep_original: bool` setting in `ProjectSettings` or `VariantConfig`
- No way to configure deletion of the original after variant generation
- No "original" pseudo-variant that could be included in `variants_json`
- If a user wants only variants (no original), they cannot configure this
- The `upload_file` endpoint (non-image) stores files with status `ready` and no variants — these are always kept as-is

---

## 5. FILE UPLOAD → PROCESSING → VARIANT GENERATION PIPELINE

### 5.1 Non-Image Upload (`POST /upload/file`)
```
Client → API Key Auth Middleware → upload_file handler
  1. Extract multipart "file" field
  2. Compute S3 key: {sanitized_project_name}-{project_id}/files/{uuid}.{ext}
  3. ensure_bucket_exists() (HEAD + create if needed + set policy)
  4. PUT object to S3 with public-read ACL
  5. INSERT file record (status="ready", variants_json={})
  6. UPDATE project storage_used_bytes += size
  7. Return FileUploadResponse { id, url, filename, mime_type, size }
```
No processing. No jobs. Immediately ready.

### 5.2 Image Upload (`POST /upload/image`)
```
Client → API Key Auth Middleware → upload_image handler
  1. Extract multipart "file" field
  2. Validate image MIME type (loose: checks content_type starts with "image/" OR extension)
  3. Parse ?variants= query param (optional comma-separated filter)
  4. Compute S3 key: {prefix}/images/original/{uuid}.{ext}
  5. ensure_bucket_exists()
  6. PUT original to S3
  7. Compute variant URLs based on project settings (filtered by ?variants if provided)
  8. INSERT file record:
     - status = "processing" (if has variants) or "ready" (if SVG or no variants)
     - variants_json = pre-computed variant URLs
  9. UPDATE project storage_used_bytes += original size
  10. IF has variants: INSERT job record (status="pending", payload={variants: config})
  11. Return ImageUploadResponse { id, original_url, variants }
```

### 5.3 Background Worker Processing
```
Worker (polling loop, semaphore-limited concurrency):
  1. claim_next_job() — BEGIN TX, SELECT ... FOR UPDATE SKIP LOCKED, update status→processing, COMMIT
  2. Broadcast "processing" status via SSE
  3. Spawn tokio task:
     a. Download original from S3 (by file.s3_key)
     b. For each variant:
        - spawn_blocking: resize/convert image using `image` crate
        - Upload processed data to S3 at variant path
        - Record variant URL
     c. UPDATE file: status="ready", variants_json = {all variant URLs}
     d. UPDATE project: transforms_used += variant_count
     e. Broadcast "completed" status via SSE
  4. On error: UPDATE job status="failed" with error in payload, broadcast
```

### 5.4 Pipeline Issues

- **Pre-computed URLs may be wrong**: Upload pre-computes variant URLs based on format config (e.g., "webp"). If the worker determines a different extension (e.g., for "original" format, it guesses from source data), the final URL may differ from what was returned to the client at upload time. The worker overwrites `variants_json`, but the client already has the old URLs.
- **No retry logic**: Failed jobs are marked "failed" with no automatic retry.
- **No dead letter queue**: Failed jobs accumulate forever.
- **No timeout**: A job can process indefinitely (e.g., decoding a huge image). The semaphore permit is held until completion.
- **Memory pressure**: The entire original image and all variant data are held in memory simultaneously. For large images (50MB upload limit) with many variants, this could consume significant RAM.
- **No progress tracking**: Multi-variant jobs report only pending/processing/completed/failed — no per-variant progress.
- **`ensure_bucket_exists` not called by worker**: The worker relies on the bucket already existing from the upload path. If uploads go through a different path (e.g., sync-variants on existing files), bucket existence is assumed.

---

## 6. API ENDPOINT COMPLETENESS

### 6.1 Endpoint Inventory

| Method | Path | Auth | Layer | Status |
|--------|------|------|-------|--------|
| GET | `/` | Public | — | ✅ Welcome HTML |
| GET | `/favicon.ico` | Public | — | ✅ Returns 204 |
| GET | `/swagger-ui` | Public | — | ✅ Swagger UI |
| POST | `/auth/login` | Public | — | ✅ |
| POST | `/auth/refresh` | Public | — | ⚠️ No new refresh token issued |
| POST | `/auth/logout` | Public | — | ✅ |
| GET | `/auth/me` | JWT | auth_middleware | ✅ |
| POST | `/users` | JWT+SU/Admin | auth+role | ✅ |
| GET | `/users` | JWT+SU/Admin | auth+role | ✅ |
| PATCH | `/users/{id}` | JWT+SU/Admin | auth+role | ⚠️ Missing from OpenAPI |
| DELETE | `/users/{id}` | JWT+SU/Admin | auth+role | ✅ |
| POST | `/projects` | JWT | auth | ✅ |
| GET | `/projects` | JWT | auth | ✅ |
| GET | `/projects/{id}` | JWT | auth | ✅ |
| PUT | `/projects/{id}` | JWT | auth | ✅ |
| DELETE | `/projects/{id}` | JWT | auth | ✅ Soft+Hard |
| POST | `/projects/{id}/restore` | JWT | auth | ✅ |
| POST | `/projects/{id}/sync-variants` | JWT | auth | ✅ |
| POST | `/projects/{id}/keys` | JWT | auth | ✅ |
| GET | `/projects/{id}/keys` | JWT | auth | ✅ |
| PATCH | `/projects/{id}/keys/{key_id}` | JWT | auth | ✅ |
| DELETE | `/projects/{id}/keys/{key_id}` | JWT | auth | ✅ |
| POST | `/upload/file` | API Key/JWT | api_key_auth | ✅ |
| POST | `/upload/image` | API Key/JWT | api_key_auth | ✅ |
| GET | `/jobs` | API Key/JWT | api_key_auth | ✅ |
| GET | `/admin/jobs` | JWT | auth | ⚠️ No role middleware |
| GET | `/admin/jobs/events` | JWT | auth | ⚠️ No role check at all |
| GET | `/files` | JWT | auth | ✅ |
| GET | `/files/{id}` | JWT | auth | ✅ |
| GET | `/files/{id}/content` | JWT | auth | ✅ Presigned redirect |
| DELETE | `/files/{id}` | JWT | auth | ✅ |

### 6.2 Missing Endpoints
- `GET /health` or `GET /ready` — health check
- `PATCH /files/{id}` — update file metadata
- `POST /upload/file` via JWT (partially exists through api_key_auth JWT fallback)
- `GET /projects/{id}/files` — list files scoped to project (must use `GET /files?project_id=`)
- `GET /projects/{id}/jobs` — list jobs scoped to project (must use `GET /jobs` with API key)
- `POST /jobs/{id}/retry` — retry a failed job
- `DELETE /jobs/{id}` — cancel/delete a job
- Any `PATCH /projects/{id}/quota` — admin endpoint to adjust quotas

### 6.3 API Design Issues
- Upload endpoints (`/upload/*`) and jobs listing (`/jobs`) use API key auth, while file management (`/files/*`) uses JWT auth. This split means external API consumers can upload but can't manage files without a JWT.
- No CORS configuration beyond `CorsLayer::permissive()` — everything is allowed.
- Rate limiting is global (120 req/60s) with no per-endpoint differentiation.
- The SSE endpoint for job events broadcasts ALL job updates to ALL subscribers — no project-level filtering.

---

## 7. SECURITY CONCERNS

1. **CorsLayer::permissive()** — allows any origin, any method, any headers. Should be restricted in production.
2. **Public-read ACL on all S3 objects** — every uploaded file is publicly accessible. No concept of private files.
3. **JWT access token lifetime is 1 hour** — IMPLEMENTATION.md says 15 minutes but code uses `chrono::Duration::hours(1)`.
4. **No password complexity requirements** for user creation.
5. **API keys are shown in plaintext** once on creation — standard practice, but there's no way to regenerate without deleting and recreating.
6. **No CSRF protection** — relies on CORS being permissive.
7. **println! statements** with user data throughout — should use `tracing` consistently (partially done).

---

## 8. SUMMARY OF HIGHEST-PRIORITY FIXES

| Priority | Issue | Impact |
|----------|-------|--------|
| P0 | BUG-2: API key `?key=` query param never works | Broken feature |
| P0 | BUG-3: Refresh token not rotated | Users forced to re-login after 1 refresh |
| P1 | BUG-6: Storage/transform quotas not enforced | Unlimited resource usage |
| P1 | BUG-12: update_user missing from OpenAPI | Hidden endpoint |
| P1 | BUG-13: SSE endpoint has no authorization | Any user sees all job updates |
| P2 | BUG-4+5: ensure_bucket_exists on every upload | Performance |
| P2 | BUG-7: Variant sizes not tracked | Inaccurate quota tracking |
| P2 | BUG-11: page=0 causes u64 underflow | User confusion |
| P2 | BUG-14: Admin jobs loads all then paginates | Memory bomb |
