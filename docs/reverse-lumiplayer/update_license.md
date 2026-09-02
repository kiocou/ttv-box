# LumiPlayer — Update / Plugin / License Reverse-Engineering (Targets ⑦⑧)

Tauri **2.11.5** desktop build. Sources: `_up_/tauri-runtime-manifest.json`, `_up_/README.txt`, `lumiplayer-tauri.exe` ASCII strings.

---

## §1 Update mechanism

**Server / endpoint (custom, NOT Tauri's built-in updater):**
- Version check: `https://download.chenfn.fun:441/lumiplayer/version.json`
- User-Agent: `LumiPlayer-Updater/1.1`
- Other referenced host: `download.chenfn.fun` / `cAnload.chenfn.fun` (likely typo-variant in one string).

**Flow:** App fetches `version.json`, compares against local `currentVersion`. If `updateAvailable`, it shows `notes`/`publishedAt` and downloads from `downloadUrl`/`download_url`. Download progress events: `update-download-progress`, `downloaded`, `percent`. On finish it reads the package and **verifies SHA-256** before applying.

**`version.json` fields observed:** `currentVersion`, `updateAvailable`, `publishedAt`, `notes`/`link`/`downloads`/`url`. Errors confirm server-provided `sha256` is mandatory and validated: `update package sha256 is required`, `update package sha256 is invalid`, `update package SHA-256 verification failed; partial file was removed`.

**Safety guards (custom updater):** `update package source is not allowed`, `update package path is not allowed`, `update package filename is invalid`, `update package url is invalid`, `update package redirected to an untrusted source`, `update package returned HTTP <code>`, `update package is too large`, `update package size does not match response`.

**`_up_` staging model:** `tauri-runtime-manifest.json` is a *build-time* artifact that copies the **playback runtime only** into `src-tauri/resources/_up_` (dirs: `mpv`, `ffmpeg`, `shaders`, `plugins`, `resources`). `README.txt` documents provenance (BtbN FFmpeg n8.1, shinchiro libmpv 2026-06-07, qBittorrent v5.2.3). `includeResources:false`, `clean:false`. Excludes cache/logs/cookies/credentials/history/locks. So `_up_` = bundled runtime resources dir shipped with the app; the live upgrade is the `version.json` custom updater above.

**Confidence:** High for endpoint, fields, SHA-256 verification, guards. **Gap:** exact `version.json` schema (signatures on the manifest itself?), how the updater relaunches/replaces the exe, and whether `_up_` runtime can be hot-patched separately from the full app package — needs dynamic capture of an actual update + network traffic.

---

## §2 Plugin system

The only `plugins/` content is **VapourSynth** — an embedded, dynamically loaded video-filter runtime, not a generic 3rd-party plugin loader.

**Layout (`plugins/vapoursynth/`):**
- Embedded Python **3.12** (`python312.dll`, `python.exe`, `python312.zip`, `python312._pth` / `python314._pth`).
- `libvapoursynth.dll` (+ `vsscript.dll`, `vspipe.exe`), `vs-plugins/`: `mvtools.dll`, `akarin/libakarin.dll`, `vsort.dll` (+ `DirectML.dll`, `onnxruntime.dll`, `models/rife_v2/rife_v4.25_lite.onnx`).
- `RUNTIME_MANIFEST.json` lists each file with `path/size/sha256` — an **integrity manifest** for the plugin bundle.
- Env hooks: `VSSCRIPT_PATH`, `VAPOURSYNTH_EXTRA_PLUGIN_PATH`; config `vapoursynth.toml`.

**Loading:** LumiPlayer invokes VapourSynth via mpv (`@lumi-interp:vapoursynth=file=[...]`, `lumi-interp VapourSynth`), exposing filters `vapoursynth-mvtools`, `vapoursynth-vsort-rife`, `performanceFallbackVapourSynth`, `gtx1050VapourSynth`. Load is dynamic (native DLL load of `libvapoursynth` → loads `vs-plugins/*.dll` + Python scripts). Errors: `PluginInitialization`, `loading failed`, `dll not found`, `superseded/replaced/timeout/busy`.

**Format of a plugin:** native VapourSynth `.dll` filter (C ABI) + optional Python module; discovered from `vs-plugins/` + `site-packages/`. No JSON plugin descriptor beyond the bundle-level `RUNTIME_MANIFEST.json`.

**Confidence:** High that VapourSynth is the plugin system and how it loads. **Gap:** whether a *generic* external plugin drop-in API exists (only VapourSynth is present in this install); exact IPC command that triggers the VS pipeline.

---

## §3 License verification (⑧)

**Storage / files (in app `data/`):** `license.dat`, `.secure-timestamp`, `.license-backup`, alongside encrypted creds (`emby/jellyfin/plex/feiniu-credentials.enc`, `cloud-accounts.json`). There is also `data/accounts/`.

**Model — hybrid offline + online:**
- **Offline:** `license.dat` + `.secure-timestamp` + `.license-backup` 三件套仍在，离线能力（本地备份、可信时间戳）的表面形态未变。**但离线校验所依赖的公钥，在静态二进制中无法定位**（见下方 §3.1 修正）。
- **Online (account activation):** IPC commands `account-activate`, `cloud-account-ac…`, `cloud-direct-status`, `cloud-account-activate` grant VIP/membership via login; server returns `expire_at`/`expires_at`/`vip_expire_at`/`pro_expire_at`.

**Tiers & state:** `is_pro`, `is_vip`, `is_member`, `is_permanent`/`forever`, `is_privilege`; expiry fields `pro_expire_at`, `vip_expire_at`, `expire_at`. So: free/trial → Pro → VIP/membership, with possible lifetime (`is_permanent`).

**Anti-tamper / integrity IPC (Tauri commands):**
`native-guard-integrity`, `pro-integrity-verify`, `pro-integrity-checksum`, `pro-verify-device`, `pro-integrity-stamp`, `pro-tamper-recalc`/`pro-tamper-sync`/`pro-tamper-status`, `pro-monitor-set-server`/`pro-monitor-set-pro`/`pro-monitor-alert`/`pro-monitor-clear`/`pro-monitor-status`, `pro-verify-status`, `device-fingerprint`, `pro-secure-save-tokens`/`pro-secure-load-tokens` (encrypted token store). `licensecleared`/`activated` are state flags.

**On failure:** license not present/invalid → features gated to free/trial; token store stays encrypted (`.enc`). No hard-crash strings found; enforcement is capability-gating + integrity monitor.

**Confidence:** High that verification is hybrid (offline artifact + online account activation) with a tamper/integrity monitor. **GAP (critical, static-scan proven): the "embedded offline RSA public key" hypothesis is NOT supported by static analysis** — see §3.1.

### §3.1 静态扫描修正（2026-08-18，推翻旧"内嵌 RSA 公钥"断言）

**结论：exe / JAR / 前端 JS 三种形态里，均无任何可解析的公钥材料。**

扫描脚本与产物（`analysis/` 下）：
- `scan_rsa.py` / `scan_rsa2.py` / `scan_rsa3.py` —— 针对 `lumiplayer-tauri.exe` + StreamHub `streamhub-local-api-0.1.0.jar`，覆盖 PEM（`BEGIN PUBLIC KEY`/`BEGIN CERTIFICATE`）、DER（`30 82 …` SEQUENCE）、base64-DER、裸 RSA 模数 / EC(P-256) 点 / Ed25519 公钥。
- `scan_frontend_keys.py` —— 扫描 `analysis/carved` + `analysis/frontend_extract` 共 69 个前端 JS 文件。
- 关键词探针：`license`/`signature`/`verify`/`rsa`/`ecdsa`/`ed25519`/`modulus`/`public_key`/`MII`/`offline`/`activation` 等周边上下文。

产物（`analysis/rsa_scan/`）：
```
rsa_keys.json            -> []      （exe+jar RSA 公钥 → 0 命中）
rsa_keys_enhanced.json   -> {"exe":[],"jar":[]}
rsa_keys_general.json    -> {"exe":[],"jar":[]}   （RSA+EC+Ed25519 通用 → 0 命中）
license_candidates.json  -> []      （license/sign/verify 上下文内无可解析密钥块 → 0 命中）
frontend_keys.json       -> []      （前端 JS 公钥 → 0 命中）
```

**含义**：
1. 旧文档 §3「binary embeds an RSA public key (hex modulus followed by ` RSA`)」**缺乏静态证据**。字符串层面曾疑似出现模数片段，但**实际可解析的公钥字节（DER/base64-DER/裸模数·点）在三个二进制里全部缺失**。
2. 离线校验公钥**不在静态二进制**中，最可能是以下之一（均需动态取证确认）：
   - (a) 由 Lumi Cloud 服务端在激活/校验时**下发**（在线通道返回，配合 `account-activate` IPC）；
   - (b) 运行时**派生 / 混淆**（如从机器码 + 常量经 KDF 生成，或经 Rust `ring` 在内存构造，无明文落盘）；
   - (c) 所谓"离线"实为**限时复核**——首次在线激活后本地缓存凭证，定期回服务端复核，并非纯本地 RSA 验签。
3. `.secure-timestamp` 仍可能是可信时间戳，但其是否对照静态公钥校验、还是对照服务端下发值，已**无法仅凭静态分析判定**。

**剩余缺口（动态，必须 runtime hook）**：
- 用 Frida / debugger 在 `license-verify`（Rust 命令，机器码拼 `"license-"+"e-verify"`）执行时**捕获实际校验用的公钥/密钥字节**；
- 抓取真实 `license.dat` + `.secure-timestamp` 字节做格式/签名算法逆向；
- 确认完整性监视器（`pro-integrity-*`）在检测到篡改时的动作（停用 vs 仅遥测）。
