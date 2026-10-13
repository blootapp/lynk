#!/usr/bin/env node
// Publishes a Lynk release to Cloudflare — the R2 bucket served by api.bloot.app:
//
//   installers + update packages  → https://api.bloot.app/download/<file>
//   lynk-latest.json              → what Lynk's built-in updater reads
//   apps/catalog.json             → what the bloot apps hub lists and installs
//
// Usage (artifacts from a GitHub Actions "Build" run):
//   node scripts/publish.mjs --run <run-id> [--notes "What's new"] [--dry-run]
//   node scripts/publish.mjs --dir <downloaded-artifacts> [...]
//
// Needs Cloudflare access to the account that runs api.bloot.app: a logged-in
// `wrangler` locally (use --cf-home <dir> to pick a wrangler login kept in its
// own folder), or CLOUDFLARE_API_TOKEN in CI. The build must have been signed with the bloot release key — unsigned
// builds are refused, because neither the updater nor the hub would accept them.

import { spawnSync } from 'node:child_process'
import { createRequire } from 'node:module'
import crypto from 'node:crypto'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const REPO = 'blootapp/lynk'
const BUCKET = 'qooti-releases'
const ACCOUNT_ID = '776b8a27d81b7766e6957af20242d7f6'
const DOWNLOAD_BASE = 'https://api.bloot.app/download'
const MANIFEST_KEY = 'lynk-latest.json'
const CATALOG_KEY = 'apps/catalog.json'
const HUB_CATALOG = path.resolve(ROOT, '..', 'bloot-apps', 'catalog', 'catalog.json')
const WRANGLER = createRequire(import.meta.url).resolve('wrangler/bin/wrangler.js')

// ── Arguments ────────────────────────────────────────────────────────────────

const args = { notes: '', dryRun: false }
for (let i = 2; i < process.argv.length; i++) {
  const a = process.argv[i]
  if (a === '--run') args.run = process.argv[++i]
  else if (a === '--dir') args.dir = process.argv[++i]
  else if (a === '--notes') args.notes = process.argv[++i] ?? ''
  else if (a === '--dry-run') args.dryRun = true
  else if (a === '--cf-home') args.cfHome = path.resolve(process.argv[++i])
  else fail(`Unknown argument: ${a}`)
}
if (!args.run && !args.dir) fail('Pass --run <github-run-id> or --dir <artifacts-dir>.')

// Every R2 call below must reach the account that runs api.bloot.app — a
// different login would quietly see an empty bucket.
const who = wrangler(['whoami'], { quiet: true })
if (!`${who.stdout}${who.stderr}`.includes(ACCOUNT_ID)) {
  fail(`The Cloudflare login can't reach the api.bloot.app account (${ACCOUNT_ID}). ` +
    'Log in with that account, or pass --cf-home <folder with that wrangler login>.')
}

const version = JSON.parse(fs.readFileSync(path.join(ROOT, 'src-tauri', 'tauri.conf.json'), 'utf8')).version
log(`Publishing Lynk ${version}${args.dryRun ? ' (dry run)' : ''}`)

// ── 1. Artifacts ─────────────────────────────────────────────────────────────

let dir = args.dir && path.resolve(args.dir)
if (!dir) {
  dir = path.join(ROOT, 'release-builds', `run-${args.run}`)
  fs.rmSync(dir, { recursive: true, force: true })
  log(`Downloading artifacts of run ${args.run}…`)
  run('gh', ['run', 'download', args.run, '-R', REPO, '-D', dir])
}
const files = walk(dir)
const find = (re, what) => files.find(f => re.test(f.replace(/\\/g, '/'))) ?? fail(`Missing ${what} in ${dir}`)

const targets = {
  'windows-x86_64': {
    install: 'nsis',
    file: find(new RegExp(`/nsis/[^/]*_${esc(version)}_x64-setup\\.exe$`), `Windows installer for ${version}`),
    key: `lynk_${version}_x64-setup.exe`,
  },
  'darwin-aarch64': {
    install: 'app-tar',
    file: find(/aarch64-apple-darwin\/macos\/[^/]+\.app\.tar\.gz$/, 'Apple Silicon update package'),
    key: `lynk_${version}_aarch64.app.tar.gz`,
  },
  'darwin-x86_64': {
    install: 'app-tar',
    file: find(/x86_64-apple-darwin\/macos\/[^/]+\.app\.tar\.gz$/, 'Intel Mac update package'),
    key: `lynk_${version}_x64.app.tar.gz`,
  },
}
const extras = [
  { file: find(new RegExp(`aarch64-apple-darwin/dmg/[^/]*_${esc(version)}_aarch64\\.dmg$`), 'Apple Silicon DMG'), key: `lynk_${version}_aarch64.dmg` },
  { file: find(new RegExp(`x86_64-apple-darwin/dmg/[^/]*_${esc(version)}_x64\\.dmg$`), 'Intel Mac DMG'), key: `lynk_${version}_x64.dmg` },
]

for (const [platform, t] of Object.entries(targets)) {
  const sig = `${t.file}.sig`
  if (!fs.existsSync(sig)) {
    fail(`${path.basename(t.file)} has no signature (.sig). The build ran without the bloot release key — ` +
      'add TAURI_SIGNING_PRIVATE_KEY (+ _PASSWORD) to the repo secrets and build again.')
  }
  t.signature = fs.readFileSync(sig, 'utf8').trim()
  t.size = fs.statSync(t.file).size
  t.sha256 = sha256(t.file)
  t.url = `${DOWNLOAD_BASE}/${t.key}`
  log(`  ${platform.padEnd(15)} ${path.basename(t.file)}  ${(t.size / 1048576).toFixed(1)} MB`)
}

// ── 2. Update manifest (Tauri updater format) ────────────────────────────────

const manifest = {
  version,
  notes: args.notes || `Lynk ${version}`,
  pub_date: new Date().toISOString().replace(/\.\d+Z$/, 'Z'),
  platforms: Object.fromEntries(Object.entries(targets).map(([p, t]) => [p, { signature: t.signature, url: t.url }])),
}

// ── 3. Hub catalogue ─────────────────────────────────────────────────────────

const tmp = path.join(dir, '.publish')
fs.mkdirSync(tmp, { recursive: true })
let catalog
const live = path.join(tmp, 'catalog.before.json')
if (wrangler(['r2', 'object', 'get', `${BUCKET}/${CATALOG_KEY}`, '--file', live], { quiet: true }).status === 0) {
  catalog = JSON.parse(fs.readFileSync(live, 'utf8'))
  log(`Live catalogue backed up to ${path.relative(ROOT, live)}`)
} else if (fs.existsSync(HUB_CATALOG)) {
  catalog = JSON.parse(fs.readFileSync(HUB_CATALOG, 'utf8'))
  log(`No live catalogue in R2 yet — starting from ${path.relative(ROOT, HUB_CATALOG)}`)
} else {
  fail('No hub catalogue found (neither in R2 nor in ../bloot-apps).')
}

const products = (catalog.products ??= [])
let lynk = products.find(p => p.id === 'lynk')
if (!lynk) {
  lynk = {
    id: 'lynk', name: 'lynk', kind: 'app',
    tagline: 'Download anything you link — fast, free, no ads.',
    requires_account: false, pricing: 'free',
    detect: {
      windows: { uninstall_key: 'Lynk', exe: 'Lynk.exe' },
      darwin: { bundle: 'Lynk.app', bundle_id: 'app.bloot.lynk' },
    },
    color: '#0338FA',
  }
  products.push(lynk)
}
delete lynk.coming_soon
lynk.platforms = Object.fromEntries(Object.entries(targets).map(([p, t]) => [p, {
  version, install: t.install, url: t.url, size: t.size, sha256: t.sha256, signature: t.signature,
}]))
catalog.updated_at = new Date().toISOString().replace(/\.\d+Z$/, 'Z')

const manifestFile = path.join(tmp, MANIFEST_KEY)
const catalogFile = path.join(tmp, 'catalog.json')
fs.writeFileSync(manifestFile, `${JSON.stringify(manifest, null, 2)}\n`)
fs.writeFileSync(catalogFile, `${JSON.stringify(catalog, null, 2)}\n`)

if (args.dryRun) {
  log(`\nDry run — nothing uploaded. Previews:\n  ${path.relative(ROOT, manifestFile)}\n  ${path.relative(ROOT, catalogFile)}`)
  process.exit(0)
}

// ── 4. Upload: files first, then the manifest, then the catalogue ────────────

const uploads = [...Object.values(targets), ...extras]
for (const u of uploads) {
  log(`↑ ${u.key}`)
  put(u.key, u.file, contentType(u.key))
}
log(`↑ ${MANIFEST_KEY}`)
put(MANIFEST_KEY, manifestFile, 'application/json')
log(`↑ ${CATALOG_KEY}`)
put(CATALOG_KEY, catalogFile, 'application/json')

// ── 5. Verify what the world now sees ────────────────────────────────────────

// The Worker's /download route answers GET only, so no HEAD requests.
const problems = []
for (const u of uploads) {
  const res = await fetch(`${DOWNLOAD_BASE}/${u.key}`)
  const len = Number(res.headers.get('content-length'))
  await res.body?.cancel()
  if (!res.ok || len !== fs.statSync(u.file).size) problems.push(`${u.key}: HTTP ${res.status}, ${len} bytes`)
}
const served = await (await fetch(`${DOWNLOAD_BASE}/${MANIFEST_KEY}`)).json()
if (served.version !== version) problems.push(`manifest reports ${served.version}`)

if (fs.existsSync(HUB_CATALOG)) {
  fs.writeFileSync(HUB_CATALOG, `${JSON.stringify(catalog, null, 2)}\n`)
  log(`Updated ${path.relative(ROOT, HUB_CATALOG)} to match.`)
}
if (problems.length) {
  // Not process.exit(): exiting while fetch sockets close crashes Node on Windows.
  console.error(`✗ Uploaded, but not served correctly:\n  ${problems.join('\n  ')}`)
  process.exitCode = 1
} else {
  log(`\n✓ Lynk ${version} is live.`)
}
log(`  Updates:   ${DOWNLOAD_BASE}/${MANIFEST_KEY}`)
for (const u of uploads) log(`  Download:  ${DOWNLOAD_BASE}/${u.key}`)

// ── Helpers ──────────────────────────────────────────────────────────────────

function put(key, file, type) {
  const r = wrangler(['r2', 'object', 'put', `${BUCKET}/${key}`, '--file', file, '--content-type', type])
  if (r.status !== 0) fail(`Upload of ${key} failed.`)
}

function wrangler(argv, { quiet = false } = {}) {
  return spawnSync(process.execPath, [WRANGLER, ...argv], {
    stdio: quiet ? 'pipe' : ['ignore', 'pipe', 'inherit'],
    env: {
      ...process.env,
      CLOUDFLARE_ACCOUNT_ID: ACCOUNT_ID,
      WRANGLER_SEND_METRICS: 'false',
      ...(args.cfHome ? { XDG_CONFIG_HOME: args.cfHome } : {}),
    },
    encoding: 'utf8',
  })
}

function run(cmd, argv) {
  const r = spawnSync(cmd, argv, { stdio: 'inherit' })
  if (r.status !== 0) fail(`${cmd} ${argv.join(' ')} failed`)
}

function walk(d) {
  return fs.readdirSync(d, { withFileTypes: true }).flatMap(e => {
    const p = path.join(d, e.name)
    return e.isDirectory() ? walk(p) : [p]
  })
}

function sha256(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex')
}

function contentType(key) {
  if (key.endsWith('.exe')) return 'application/vnd.microsoft.portable-executable'
  if (key.endsWith('.dmg')) return 'application/x-apple-diskimage'
  if (key.endsWith('.tar.gz')) return 'application/gzip'
  return 'application/octet-stream'
}

function esc(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

function log(msg) {
  console.log(msg)
}

function fail(msg) {
  console.error(`✗ ${msg}`)
  process.exit(1)
}
