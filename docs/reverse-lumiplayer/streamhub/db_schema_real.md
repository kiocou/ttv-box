# LumiPlayer — Recovered Real SQLite Schema (TARGET ④)

Recovered **statically** from the shipped artifacts (no runtime `*.db` exists on disk).

- **streamhub.db** DDL → from `BOOT-INF/classes/schema.sql` inside the Spring Boot fat JAR, plus runtime `CREATE INDEX` constants found in `DatabaseMigrationRunner` bytecode.
- **lumi-store.db** DDL → from raw SQL string literals embedded in `lumiplayer-tauri.exe` (captured in `analysis/strings_ascii.txt`, lines 199374–199399).
- **Confidence legend:** HIGH = DDL taken verbatim from a packaged/embedded resource; MED = inferred/derived.

---

## Section A — `streamhub.db` (StreamHub / Spring Boot side)

**Connection:** `jdbc:sqlite:./data/streamhub.db?busy_timeout=30000` (driver `org.sqlite.JDBC`, Hikari `maximum-pool-size: 1`). ORM = MyBatis-Plus (`id-type: auto` ⇒ `AUTOINCREMENT`, `map-underscore-to-camel-case: true`). **No `FOREIGN KEY` constraints are declared anywhere** — relationships are logical only (see §A.16).

### A.0 Source tables actually present (15)
> **Correction vs. prior hypothesis:** the real DB does **NOT** contain `library`, `subtitle`, or `play_history` tables. `subtitle_paths` is merely a column on `media_file`. The real tables are 15 (13 have a matching `*Entity` class in `entities.md`; the other 2 — `scrape_cache`, `agent_shortcut_click` — are also in `schema.sql`).

`media_source, movie, tv_show, tv_episode, media_file, scrape_cache, watch_history, agent_feedback, agent_preference_signal, agent_shortcut_click, app_setting, users, refresh_tokens, email_verifications, password_resets`

---

### A.1 `media_source`
```sql
CREATE TABLE IF NOT EXISTS media_source (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    type TEXT NOT NULL DEFAULT 'webdav',
    url TEXT NOT NULL,
    username TEXT,
    encrypted_password TEXT,
    root_path TEXT NOT NULL,
    scan_interval_minutes INTEGER NOT NULL DEFAULT 30,
    enabled INTEGER NOT NULL DEFAULT 1,
    enable_scheduled_sync INTEGER NOT NULL DEFAULT 1,
    connection_status TEXT NOT NULL DEFAULT 'UNKNOWN',
    last_sync_time TEXT,
    last_sync_status TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```
PK: `id`. FK: none. Indexes: (runtime) `uq_media_file_source_remote_path` references it; cleanup `DELETE FROM media_file WHERE source_id NOT IN (SELECT id FROM media_source)`. **Confidence: HIGH** (schema.sql).

### A.2 `movie`
```sql
CREATE TABLE IF NOT EXISTS movie (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tmdb_id INTEGER,
    title TEXT NOT NULL,
    original_title TEXT,
    year INTEGER,
    overview TEXT,
    poster_path TEXT,
    backdrop_path TEXT,
    logo_path TEXT,
    rating REAL, rating_votes INTEGER,
    imdb_rating REAL, imdb_votes INTEGER,
    trakt_rating REAL, trakt_votes INTEGER,
    rotten_tomatoes_rating REAL, rotten_tomatoes_votes INTEGER,
    external_ratings_updated_at TEXT,
    runtime_minutes INTEGER,
    tagline TEXT, certification TEXT, release_date TEXT,
    content_category TEXT NOT NULL DEFAULT 'movie',
    genres_json TEXT, origin_countries_json TEXT, spoken_languages_json TEXT,
    directors_json TEXT, production_companies_json TEXT, cast_json TEXT,
    budget INTEGER, revenue INTEGER,
    imdb_id TEXT, instagram_id TEXT, facebook_id TEXT,
    collection_name TEXT, collection_items_json TEXT, recommendations_json TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```
PK: `id`. FK: none. Indexes: `idx_movie_tmdb_id` (schema.sql), `idx_movie_title_year_category`, `idx_movie_category_created_at`, `idx_movie_updated_at` (schema.sql). **Runtime-added (runner):** `uq_movie_tmdb_id` `UNIQUE(tmdb_id) WHERE tmdb_id IS NOT NULL`; `uq_movie_title_year_category` `UNIQUE(title, ifnull(year,-1), content_category) WHERE tmdb_id IS NULL`.
**Confidence: HIGH** (schema.sql + runner constants).

### A.3 `tv_show`
Identical shape to `movie` but with `total_seasons INTEGER` (instead of `budget/revenue`) and `content_category TEXT NOT NULL DEFAULT 'tv'`. No `imdb_id/instagram_id/facebook_id` differs? — same social ids present.
```sql
CREATE TABLE IF NOT EXISTS tv_show (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tmdb_id INTEGER, title TEXT NOT NULL, original_title TEXT, year INTEGER,
    overview TEXT, poster_path TEXT, backdrop_path TEXT, logo_path TEXT,
    rating REAL, rating_votes INTEGER,
    imdb_rating REAL, imdb_votes INTEGER,
    trakt_rating REAL, trakt_votes INTEGER,
    rotten_tomatoes_rating REAL, rotten_tomatoes_votes INTEGER,
    external_ratings_updated_at TEXT,
    runtime_minutes INTEGER, tagline TEXT, certification TEXT, release_date TEXT,
    total_seasons INTEGER,
    content_category TEXT NOT NULL DEFAULT 'tv',
    genres_json TEXT, origin_countries_json TEXT, spoken_languages_json TEXT,
    directors_json TEXT, production_companies_json TEXT, cast_json TEXT,
    imdb_id TEXT, instagram_id TEXT, facebook_id TEXT,
    collection_name TEXT, collection_items_json TEXT, recommendations_json TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```
PK: `id`. Indexes: `idx_tv_show_tmdb_id`, `idx_tv_show_title_year_category`, `idx_tv_show_category_created_at`, `idx_tv_show_updated_at` (schema.sql). **Runtime:** `uq_tv_show_tmdb_id` `UNIQUE(tmdb_id) WHERE tmdb_id IS NOT NULL`; `uq_tv_show_title_year_category` `UNIQUE(title, ifnull(year,-1), content_category) WHERE tmdb_id IS NULL`. **Confidence: HIGH**.

### A.4 `tv_episode`
```sql
CREATE TABLE IF NOT EXISTS tv_episode (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    show_id INTEGER NOT NULL,
    season_number INTEGER NOT NULL,
    episode_number INTEGER NOT NULL,
    title TEXT NOT NULL,
    overview TEXT,
    still_path TEXT,
    duration_seconds INTEGER,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```
PK: `id`. Indexes: `idx_tv_episode_show_season_episode` `UNIQUE(show_id, season_number, episode_number)` (schema.sql). Logical FK: `show_id → tv_show.id`. **Confidence: HIGH**.

### A.5 `media_file`
```sql
CREATE TABLE IF NOT EXISTS media_file (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id INTEGER,
    media_type TEXT NOT NULL,
    media_ref_id INTEGER NOT NULL,
    remote_path TEXT NOT NULL,
    file_name TEXT NOT NULL,
    file_size INTEGER,
    etag TEXT,
    last_modified TEXT,
    subtitle_paths TEXT,
    scrape_status TEXT NOT NULL DEFAULT 'SCRAPED',
    scrape_quality TEXT NOT NULL DEFAULT 'COMPLETE',
    scrape_reason_codes TEXT,
    missing_metadata_fields TEXT,
    last_scraped_at TEXT, last_attempt_at TEXT, next_retry_at TEXT,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    failure_code TEXT, scrape_failure_trace TEXT,
    parser_version TEXT, matcher_version TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```
PK: `id`. Indexes (schema.sql): `idx_media_file_source_id(source_id)`, `idx_media_file_remote_path(remote_path)`, `idx_media_file_media_ref(media_type, media_ref_id)`, `idx_media_file_next_retry(scrape_status, next_retry_at)`. **Runtime:** `uq_media_file_source_remote_path` `UNIQUE(ifnull(source_id,-1), remote_path)`. Logical FK: `source_id → media_source.id`; `media_ref_id → movie.id (media_type='movie')` or `tv_episode.id (media_type='episode')`. **Confidence: HIGH**.

### A.6 `scrape_cache`
```sql
CREATE TABLE IF NOT EXISTS scrape_cache (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    cache_type TEXT NOT NULL,
    cache_key TEXT NOT NULL,
    cache_value TEXT NOT NULL,
    expires_at TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```
Indexes: `uq_scrape_cache_type_key` `UNIQUE(cache_type, cache_key)` (schema.sql). **Confidence: HIGH**.

### A.7 `watch_history`
```sql
CREATE TABLE IF NOT EXISTS watch_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    media_id INTEGER NOT NULL,
    media_type TEXT NOT NULL,
    media_file_id INTEGER,
    progress_seconds INTEGER NOT NULL DEFAULT 0,
    last_watched_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```
Indexes (schema.sql): none declared for this table in schema.sql. **Runtime-added (runner):** `idx_watch_history_user_media` `UNIQUE(user_id, media_id, media_type)`; `idx_watch_history_user_last_watched_at(user_id, last_watched_at DESC)`. Logical FK: `user_id → users.id`; `media_id → movie.id (media_type='movie')` / `tv_episode.id` (otherwise). **Confidence: HIGH**.

### A.8 `agent_feedback`
```sql
CREATE TABLE IF NOT EXISTS agent_feedback (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    query_text TEXT, feedback_type TEXT NOT NULL, note TEXT, source TEXT,
    local_available INTEGER NOT NULL DEFAULT 0, local_id INTEGER,
    media_type TEXT, tmdb_id INTEGER, title TEXT,
    genres_json TEXT, regions_json TEXT, duration_seconds INTEGER,
    reason_tags_json TEXT, task_type TEXT, candidate_pool TEXT,
    gate_status TEXT, evidence_json TEXT, route_sources_json TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```
Indexes: `idx_agent_feedback_user_created(user_id, created_at DESC)` (schema.sql + runner). Logical FK: `user_id → users.id`. **Confidence: HIGH**.

### A.9 `agent_preference_signal`
```sql
CREATE TABLE IF NOT EXISTS agent_preference_signal (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    source_feedback_id INTEGER,
    signal_type TEXT NOT NULL, target_type TEXT NOT NULL, target_key TEXT NOT NULL, target_value TEXT,
    polarity INTEGER NOT NULL DEFAULT 0,
    weight REAL NOT NULL DEFAULT 0, confidence REAL NOT NULL DEFAULT 1,
    task_scope TEXT NOT NULL DEFAULT 'ANY', query_scope TEXT,
    evidence_bound INTEGER NOT NULL DEFAULT 0,
    decay_half_life_days INTEGER NOT NULL DEFAULT 180,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```
Indexes: `idx_agent_preference_signal_user_scope(user_id, signal_type, task_scope, created_at DESC)` (schema.sql + runner). Logical FK: `user_id → users.id`, `source_feedback_id → agent_feedback.id`. **Confidence: HIGH**.

### A.10 `agent_shortcut_click`
```sql
CREATE TABLE IF NOT EXISTS agent_shortcut_click (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    shortcut_id TEXT NOT NULL, label TEXT NOT NULL, query_text TEXT NOT NULL,
    shortcut_source TEXT, task_type TEXT, structured_query_snapshot TEXT,
    cache_hit INTEGER NOT NULL DEFAULT 0,
    clicked_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```
Indexes: `idx_agent_shortcut_click_user_time(user_id, clicked_at DESC)` (schema.sql + runner). Logical FK: `user_id → users.id`. **Confidence: HIGH**.

### A.11 `app_setting`
```sql
CREATE TABLE IF NOT EXISTS app_setting (
    setting_key TEXT PRIMARY KEY,
    setting_value TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```
PK: `setting_key` (TEXT, non-auto). FK: none. **Confidence: HIGH**.

### A.12 `users`
```sql
CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL, email TEXT NOT NULL, password_hash TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'user',
    status TEXT NOT NULL DEFAULT 'pending_verification',
    email_verified INTEGER NOT NULL DEFAULT 0,
    token_version INTEGER NOT NULL DEFAULT 0,
    failed_login_count INTEGER NOT NULL DEFAULT 0,
    locked_until TEXT, last_login_at TEXT, last_login_ip TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at TEXT
);
```
Indexes (schema.sql): `idx_users_username` `UNIQUE(username)`, `idx_users_email` `UNIQUE(email)`, `idx_users_role_status(role, status)`. Parent of all `*_user_id` FKs. **Confidence: HIGH**.

### A.13 `refresh_tokens`
```sql
CREATE TABLE IF NOT EXISTS refresh_tokens (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    token_hash TEXT NOT NULL,
    device_name TEXT, user_agent TEXT, ip_address TEXT,
    expires_at TEXT NOT NULL, revoked_at TEXT, replaced_by_token_id INTEGER,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_used_at TEXT
);
```
Indexes (schema.sql): `idx_refresh_tokens_token_hash` `UNIQUE(token_hash)`, `idx_refresh_tokens_user_revoked(user_id, revoked_at)`. Logical FK: `user_id → users.id`. **Confidence: HIGH**.

### A.14 `email_verifications`
```sql
CREATE TABLE IF NOT EXISTS email_verifications (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL DEFAULT 0,
    email TEXT NOT NULL, token_hash TEXT NOT NULL, code_hash TEXT,
    purpose TEXT NOT NULL DEFAULT 'register_verify',
    expires_at TEXT NOT NULL, used_at TEXT, sent_at TEXT,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    created_ip TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```
Indexes (schema.sql): `idx_email_verifications_token_hash` `UNIQUE(token_hash)`, `idx_email_verifications_user_purpose(user_id, purpose, used_at)`. Logical FK: `user_id → users.id`. **Confidence: HIGH**.

### A.15 `password_resets`
```sql
CREATE TABLE IF NOT EXISTS password_resets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL, email TEXT NOT NULL, token_hash TEXT NOT NULL,
    expires_at TEXT NOT NULL, used_at TEXT, created_ip TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```
Indexes (schema.sql): `idx_password_resets_token_hash` `UNIQUE(token_hash)`, `idx_password_resets_user_used(user_id, used_at)`. Logical FK: `user_id → users.id`. **Confidence: HIGH**.

### A.16 Logical (unenforced) relationships — derived from `DatabaseMigrationRunner` orphan-cleanup SQL
```
media_file.source_id            -> media_source.id
media_file.media_ref_id        -> movie.id            (media_file.media_type='movie')
media_file.media_ref_id        -> tv_episode.id       (media_file.media_type='episode')
tv_episode.show_id             -> tv_show.id
watch_history.user_id          -> users.id
watch_history.media_id         -> movie.id | tv_episode.id   (by media_type)
agent_feedback.user_id         -> users.id
agent_preference_signal.user_id-> users.id
agent_preference_signal.source_feedback_id -> agent_feedback.id
agent_shortcut_click.user_id   -> users.id
refresh_tokens.user_id         -> users.id
email_verifications.user_id    -> users.id
password_resets.user_id        -> users.id
```
No `REFERENCES`/`FOREIGN KEY` clauses exist; integrity is maintained in application code.

### A.17 Full index inventory (schema.sql + runtime)
schema.sql: `idx_media_file_source_id, idx_media_file_remote_path, idx_media_file_media_ref, idx_media_file_next_retry, uq_scrape_cache_type_key, idx_movie_tmdb_id, idx_movie_title_year_category, idx_movie_category_created_at, idx_movie_updated_at, idx_tv_show_tmdb_id, idx_tv_show_title_year_category, idx_tv_show_category_created_at, idx_tv_show_updated_at, idx_tv_episode_show_season_episode, idx_users_username, idx_users_email, idx_users_role_status, idx_refresh_tokens_token_hash, idx_refresh_tokens_user_revoked, idx_email_verifications_token_hash, idx_email_verifications_user_purpose, idx_password_resets_token_hash, idx_password_resets_user_used, idx_agent_feedback_user_created, idx_agent_preference_signal_user_scope, idx_agent_shortcut_click_user_time`.

Runtime-added by `DatabaseMigrationRunner` (constants in bytecode, idempotent `CREATE ... IF NOT EXISTS`): `uq_media_file_source_remote_path UNIQUE(media_file(ifnull(source_id,-1), remote_path))`, `idx_watch_history_user_media UNIQUE(watch_history(user_id, media_id, media_type))`, `idx_watch_history_user_last_watched_at(watch_history(user_id, last_watched_at DESC))`, `uq_movie_tmdb_id UNIQUE(movie(tmdb_id)) WHERE tmdb_id IS NOT NULL`, `uq_movie_title_year_category UNIQUE(movie(title, ifnull(year,-1), content_category)) WHERE tmdb_id IS NULL`, `uq_tv_show_tmdb_id UNIQUE(tv_show(tmdb_id)) WHERE tmdb_id IS NOT NULL`, `uq_tv_show_title_year_category UNIQUE(tv_show(title, ifnull(year,-1), content_category)) WHERE tmdb_id IS NULL`.

---

## Section B — `lumi-store.db` (Rust / Tauri side)

**Source:** `analysis/strings_ascii.txt` lines 199374–199399 (verbatim SQL literals from `lumiplayer-tauri.exe`). Applied by Rust with PRAGMAs `journal_mode=WAL; synchronous=NORMAL; foreign_keys=ON`. **Confidence: HIGH** (literal SQL in the binary).

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS kv (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS media (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    library_id TEXT NOT NULL DEFAULT '',
    kind       TEXT NOT NULL DEFAULT '',
    title      TEXT NOT NULL DEFAULT '',
    sort_key   TEXT NOT NULL DEFAULT '',
    year       INTEGER,
    art_url    TEXT,
    payload    TEXT,
    updated_at INTEGER NOT NULL
);

-- Ordered exactly as the library pages: filter columns first, then the sort
-- column, so a page is an index range scan with no sort.
CREATE INDEX IF NOT EXISTS idx_media_page
    ON media(account_id, library_id, kind, sort_key, id);

CREATE VIRTUAL TABLE IF NOT EXISTS media_fts
    USING fts5(title, id UNINDEXED, tokenize = 'unicode61');
```

Notes:
- `media.id` and `kv.key` are **TEXT primary keys** (not AUTOINCREMENT) — content-addressed.
- FTS5 table `media_fts` indexes `title` (searchable) with `id` carried as `UNINDEXED`, `unicode61` tokenizer. Sync is manual: binary strings show `INSERT INTO media_fts(title, id) VALUES(?1, ?2)` and `DELETE FROM media_fts WHERE id = ?1` on media write/delete.
- Confirmed by surrounding strings: `DELETE FROM media_fts WHERE id IN (SELECT id FROM media WHERE account_id = ?1)` (account purge), and search via `... FROM media WHERE id IN (SELECT id FROM media_fts WHERE media_fts MATCH ?1) ORDER BY sort_key ASC LIMIT ?2`.
- No separate `library`/`play_history` tables on this side either; `library_id` is a column on `media`.

---

## Section C — How the schema is applied at runtime

### C.1 streamhub.db
1. **`application.yml`** sets `spring.sql.init.mode: always` and `spring.datasource.url=jdbc:sqlite:./data/streamhub.db?busy_timeout=30000`.
2. On startup, Spring Boot's `DataSourceInitializer` executes the classpath resource **`schema.sql`** (the §A DDL) — `CREATE TABLE IF NOT EXISTS ...` + base indexes, so it is idempotent and safe across restarts. (No `data.sql`/`import.sql` exists in the JAR ⇒ no seeded rows such as a default admin.)
3. Then `DatabaseMigrationRunner` (a Spring `ApplicationRunner`, constructor-injected with `JdbcTemplate`, `ImageStorageService`, `LibraryService`, `FileNameParser`) runs `run(ApplicationArguments)`:
   - Executes **runtime `CREATE UNIQUE/INDEX IF NOT EXISTS`** statements (§A.17) — partial unique indexes for de-dup.
   - Runs **backward-compatibility `ALTER TABLE … ADD COLUMN`** migrations — these are built dynamically (table/column/type concatenated), so literal DDL is not a string constant; they only add columns that already exist verbatim in `schema.sql`, i.e. they target **old** DB files. A fresh install already has them from `schema.sql`.
   - Runs **data-repair `UPDATE`/`DELETE`** passes: backfills `content_category`, `origin_countries_json='[]'`, `enable_scheduled_sync`, `connection_status`; recomputes `media_file.scrape_status/quality`; and **orphan cleanup** that deletes rows whose parent is missing (revealing the §A.16 logical FKs).
4. **No Flyway / Liquibase** is used. **No FK constraints** are declared; `sqlite-jdbc-3.49.1.0` is the driver. Hikari pool size = 1 (single-writer SQLite).

### C.2 lumi-store.db
Rust opens `lumi-store.db`, sets `journal_mode=WAL; synchronous=NORMAL; foreign_keys=ON`, then issues the §B `CREATE TABLE` / `CREATE INDEX` / `CREATE VIRTUAL TABLE` statements (all `IF NOT EXISTS`), and maintains `media_fts` via explicit INSERT/DELETE on every media mutation. Logic lives in `00-lumi-store.js` (renderer) / Rust datastore (`datastore: … failed` error strings present).

---

## Summary of sources & confidence

| DB | Table | Source | Confidence |
|----|-------|--------|-----------|
| streamhub | all 15 tables | `BOOT-INF/classes/schema.sql` (JAR) | HIGH |
| streamhub | 7 extra indexes | `DatabaseMigrationRunner` bytecode constants | HIGH |
| streamhub | logical FKs | runner cleanup SQL | HIGH (logical) / NONE (declared) |
| lumi-store | kv, media, media_fts, idx, PRAGMA | exe string literals (`strings_ascii.txt` 199374–199399) | HIGH |

**Key correction for 1:1 rebuild:** discard the earlier `library`/`subtitle`/`play_history` assumption — those tables do not exist. Use the 15-table `streamhub.db` schema above and the 3-object `lumi-store.db` schema.
