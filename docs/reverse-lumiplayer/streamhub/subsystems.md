# StreamHub Subsystems (Target ⑪–⑭)

Reverse-engineered from `streamhub-local-api-0.1.0.jar` (javap `-p -c` on extracted `BOOT-INF/classes`).
ffmpeg comes from **JAVE2** `ws.schild.jave.process.ffmpeg.DefaultFFMPEGLocator` (bundled binary).

---

## §1 Stream proxy / HLS  (⑬)

Class `service.impl.StreamProxyServiceImpl`. HLS is **fMP4 segmented** (`-hls_segment_type fmp4`), 6s segments, infinite list.

**ffmpeg base args** — `createFfmpegProcessBuilder(source, remotePath, ...varargs)`:
`ffmpeg -hide_banner -loglevel <lvl> -rw_timeout 15000000 -analyzeduration 10000000 -probesize 10000000 -i <buildAuthenticatedUrl(source, remotePath)> {varargs}`

**Video playlist** — `startVideoProcessIfNeeded` (one global video m3u8):
`-map 0:v:0` ; transcode branch `-c:v libx264 -preset veryfast -crf 21 -pix_fmt yuv420p -movflags +faststart` else `-c:v copy` ; then `-an -sn -f hls -hls_time 6 -hls_list_size 0 -hls_flags independent_segments+append_list -hls_segment_type fmp4 -hls_fmp4_init_filename init.mp4 -hls_segment_filename <dir>/segment_%05d.m4s <video/index.m3u8>`.
Working dir = video dir; stdout+stderr `DISCARD`; `Process` cached in `hlsProcessMap` keyed by mediaFileId.

**Audio playlist** — `startAudioProcessIfNeeded` (one per audio track):
`-map <idx> -vn -c:a aac -ac 2 -b:a 192k -f hls -hls_time 6 -hls_list_size 0 -hls_flags independent_segments+append_list -hls_segment_type fmp4 -hls_fmp4_init_filename init.mp4 -hls_segment_filename <dir>/segment_%05d.m4s <audioPlaylistPath>`.

**Master playlist** — `buildMasterPlaylist` → `createMasterPlaylist` writes `master.m3u8`:
`#EXTM3U` / `#EXT-X-VERSION:7` / `#EXT-X-INDEPENDENT-SEGMENTS` ; per audio track `#EXT-X-MEDIA:TYPE=AUDIO,...,GROUP-ID="audio",LANGUAGE=<norm|und>,NAME=<label>,AUTOSELECT=YES(NO for non-default)` (attrs via `escapeAttribute`); then `#EXT-X-STREAM-INF:BANDWIDTH=8000000,AUDIO="audio"` + `video/index.m3u8`. Asset URLs via `hlsAssetUrl` = `…/hls/{id}/{URLEncoder.encode(relPath,UTF_8)}`.

**Subtitle extraction** — `ensureEmbeddedSubtitle`: `-map <subIdx> -c:s webvtt -f webvtt -y <out>` (output in `subtitlesDirectory`).
`inspectPlaybackDetails` parses ffprobe-style text with regex `Stream #\d+:(\d+)(?:\(([^)]+)\))?: (Video|Audio|Subtitle):\s*(...)` to build `PlaybackDetails` → `PlaybackPlan{useHlsPlayback, transcodeVideo, videoCodec(h264/avc1), audioCodec(aac/mp4a)}`.
`resolveHlsFile` guards path traversal (`startsWith(hlsRootDirectory)`) and missing segments. Public API: `openVideo`, `openSubtitle`, `resolvePlayableUri`, `describeVideo`, `getTrackInfo`, `ensureHlsPlaylist`.

---

## §2 RAG / AI wiring  (⑫) — OPTIONAL / NOT WIRED

Config exists in `StreamHubProperties.Rag`:
- `Qdrant{enabled, baseUrl=http://localhost:6333, apiKey, collection=streamhub_media}`
- `Meilisearch{enabled, baseUrl=http://localhost:7700, apiKey, index=streamhub_media}`
- `Embedding{enabled, baseUrl=https://api.openai.com/v1, apiKey, model=text-embedding-3-small, dimensions}`
- `Reranker{enabled, baseUrl, apiKey, model, topN=12}`

**Reality:** No `io.qdrant` or `meilisearch` client dependency is referenced anywhere in the bytecode (only the config classes + `RagIndexResponse` DTO + `SettingsController` config endpoint). The recommendation execution path (`RecommendationAgentServiceImpl` → `RagFusionStrategy` → `TaskAwareReranker`) is an **in-process** retrieval/fusion/rerank over cached candidate pools: `QueryFingerprintService`, `RepeatRequestHandler`, `AgentCacheDecisionEngine` (`USE_FINAL_RESULT_CACHE` / `USE_CANDIDATE_POOL_AND_RERANK` / `USE_PREPARED_PLAN_AND_RECALL`), `AgentResponseCacheService`, `CandidatePoolCacheService`. It does **not** embed queries, does **not** call OpenAI, does **not** query Qdrant/Meilisearch, does **not** call an external reranker. "RAG" is a misnomer — it is cached local retrieval + fusion + rerank. Vector store / embedding / reranker are declared-but-inactive config.

---

## §3 Metadata scraping & image backfill  (⑭)

`config.ImageBackfillRunner` (`ApplicationRunner`): on boot backfills `Movie`/`TvShow`/`TvEpisode` images through `TmdbService` → `ImageStorageService` (cache root `/cache/images/`).
Flow: `backfillMovies/Shows/Episodes` → `needsMovieRefresh`/`needsShowRefresh` (checks `hasMissingCachedImage`/`hasMissingCastImage`/`hasMissingProductionCompanyLogo`/`hasMissingRelatedMediaPoster`) → `refreshMovieImages`/`ShowImages` → `applyMovieImages`/`applyShowImages` → `evictLibraryCaches()`.
TMDB aggregation (`TmdbServiceImpl` → `TmdbMetadata`): poster, backdrop, `toCast` (PersonCredit), `toProductionCompanies`, `toRelatedMedia`; `localize*` helpers i18n-localize cast / backdrop / production / related.
**MdbList** (`MdbListRatingServiceImpl` + `StreamHubProperties.MdbList`): ratings aggregation — a separate concern from artwork.

**Thumbnail extraction:** There is **no ffprobe / thumbnail-extraction code in StreamHub** (no ffprobe reference anywhere). Thumbnails originate from Emby/Jellyfin/Plex image endpoints or TMDB. StreamHub only does ffmpeg **embedded-subtitle extraction (webvtt)** and **HLS transcode** locally.

---

## §4 Media-server negotiation  (⑪) — Rust/Tauri side

Lives in the Rust command layer (`rust_command_layer.md` §13), not in this JAR. Full field set exchanged:

**Emby / Jellyfin** — `GET /Items/{itemGuid}/PlaybackInfo` with params `api_key, UserId, EnableDirectPlay/EnableDirectStream/EnableTranscoding, MediaSourceId, PlaySessionId`. Takes `MediaSources[].DirectStreamUrl`; `RequiredHttpHeaders` (e.g. `X-Emby-Token`) stored into `embyRequiredHttpHeaders`. Auth header set built from obfuscated (`H3`/`H3O`/`H9`) XOR string table: `x-emby-authorization, X-Emby-Token, x-emby-token, x-emby-token…`. Connection config: `embyServerUrl, embyAccessToken, embyUserId, embyDeviceId, privateGatewayMode, proxyMode, mediaProxyMode, auth_mode`. Flags: `forceEmbyPlaybackInfo`, `fastStart`. Timers: `playbackInfoMs, redirectMs, plexMs`. Outcomes: `cache-hit | renderer-confirmed-source | fast-start-raw-url`.

**Plex** — `GET /library/metadata/{ratingKey}?X-Plex-Token=<token>` → parse `MediaContainer`→`MediaMetadataPart`, take `DirectStreamUrl`. `ratingKey`/`_plexRatingKey` media key; token from Plex credentials (`.enc`, DPAPI/account-bound, same scheme as `CredentialCryptoService`).

**Routing:** remote Emby → system-proxy media path (`mediaProxyMode`); media-server → local direct proxy (`StreamHub :port/stream` or Rust local proxy). Bypass via `lumi_bypass_proxy_emby` / `media_bypass_proxy`. Result fields returned to frontend: `guid, title, itemNo, playable, mediaType, seasonNumber, episodeNumber, rawUrl, originalUrl, poster, backdrop, overview, duration, httpHeaders, redirect, outcome, cache-hit, playbackInfoMs, redirectMs, plexMs, resolvedSource, thumbnailExtraction`.
