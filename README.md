# Lynk

Paste a link, get the file — video (MP4/MKV) or audio (MP3/M4A) from YouTube, TikTok, Instagram, X and 1,800+ other sites. A [bloot](https://bloot.app) app for Windows and macOS, built with Tauri 2 on top of [yt-dlp](https://github.com/yt-dlp/yt-dlp) and [ffmpeg](https://ffmpeg.org).

## Develop

```bash
npm ci
bash scripts/fetch-sidecars.sh   # yt-dlp + ffmpeg for this machine
npm run tauri dev
```

`npm run dev` alone serves the UI in a browser at http://localhost:1430 against a mock backend.

## Release

1. Bump the version in `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and `package.json`.
2. Commit, then tag and push: `git tag v0.1.1 && git push origin main v0.1.1`.
3. GitHub Actions builds signed installers for Windows and macOS (Apple Silicon + Intel).
4. Publish to Cloudflare — the app's auto-updater and the bloot apps hub pick it up:
   - automatically, when the `CLOUDFLARE_API_TOKEN` secret is set, or
   - locally: `node scripts/publish.mjs --run <run-id>` (add `--dry-run` to preview).

Files are served from `https://api.bloot.app/download/` (R2 bucket `qooti-releases`):
`lynk-latest.json` for the updater and `apps/catalog.json` for the hub.

Repo secrets: `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (the bloot release key),
optionally `CLOUDFLARE_API_TOKEN` (R2 edit access) for automatic publishing.
