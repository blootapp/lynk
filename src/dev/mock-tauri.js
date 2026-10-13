// Dev-only mock backend so the UI can be developed in a plain browser
// (`npm run dev`, then open http://localhost:1420). Never bundled in builds:
// main.js only imports it behind `import.meta.env.DEV`.
//
// Links containing "list=" or "playlist" become playlists; "fail" fails.
import { mockIPC, mockWindows } from '@tauri-apps/api/mocks'
import { emit } from '@tauri-apps/api/event'

mockWindows('main')

const prefs = {
  default_quality: '1080p', default_format: 'video', audio_format: 'mp3', video_format: 'mp4', ask_save_path: 'false',
  prefer_h264: 'true', max_concurrent: '3', notifications: 'true', ytdlp_auto_update: 'true',
  cookies_browser: '', save_path: 'C:\\Users\\you\\Downloads\\Lynk', autostart_initialized: 'true',
}
const now = Date.now()
const H = 3600e3
const yt = id => `https://i.ytimg.com/vi/${id}/mqdefault.jpg`
const base = { quality: '1080p', audio_format: 'mp3', video_format: 'mp4', prefer_h264: true, progress: 100, save_path: prefs.save_path }
const db = new Map([
  { id: 'h1', url: 'https://www.youtube.com/watch?v=aqz-KE-bpKQ', title: 'Big Buck Bunny 60fps 4K - Official Blender Foundation Short Film', thumbnail: yt('aqz-KE-bpKQ'), duration: 635, site: 'Youtube', format: 'video', ext: 'mp4', status: 'done', size_bytes: 248_512_000, created_at: now - 2 * H, finished_at: now - 2 * H + 90e3 },
  { id: 'h2', url: 'https://soundcloud.com/x/y', title: 'Lofi beats to study to — Vol. 3', site: 'Soundcloud', format: 'audio', ext: 'mp3', status: 'done', size_bytes: 8_734_000, duration: 214, created_at: now - 5 * H, finished_at: now - 5 * H + 20e3 },
  { id: 'h3', url: 'https://www.instagram.com/reel/abc', title: null, site: 'Instagram', format: 'video', ext: 'mp4', status: 'error', progress: 0, error_msg: 'This content requires signing in. Pick your browser under Settings › Sign-in cookies.', error_detail: 'Requested content is not available, rate-limit reached or login required', created_at: now - 26 * H, finished_at: now - 26 * H },
  { id: 'h4', url: 'https://www.youtube.com/watch?v=eRsGyueVLvQ', title: 'Sintel — Open Movie by Blender Foundation', thumbnail: yt('eRsGyueVLvQ'), duration: 888, site: 'Youtube', format: 'video', ext: 'mp4', status: 'done', size_bytes: 412_300_000, file_missing: true, created_at: now - 4 * 24 * H, finished_at: now - 4 * 24 * H },
].map(d => [d.id, { ...base, ...d }]))
const playlists = new Map()
const queue = []
const running = new Set()
let clipboard = 'https://www.youtube.com/watch?v=YE7VzlLtp-4'
let appUpdate = { state: 'current' }
let engine = { version: '2026.06.09', updating: false, last_check: now - 30 * H, error: null, updated_to: null }

window.__mock = { db, setClipboard: v => { clipboard = v } }

const sleep = ms => new Promise(r => setTimeout(r, ms))
const push = d => emit('download:update', { ...d })
const uid = () => Math.random().toString(36).slice(2, 10)

function pump() {
  while (running.size < Number(prefs.max_concurrent) && queue.length) {
    const id = queue.shift()
    running.add(id)
    simulate(id).finally(() => { running.delete(id); pump() })
  }
}

async function simulate(id) {
  const d = db.get(id)
  const alive = () => db.get(id) === d && !d._cancel
  Object.assign(d, { status: 'fetching', stage: 'fetching' }); push(d)
  await sleep(900 + Math.random() * 600)
  if (!alive()) return finishCancel(d)

  if (!d.group_id && /list=|playlist/.test(d.url)) {
    const n = 12
    playlists.set(id, Array.from({ length: n }, (_, i) => ({ index: i + 1, title: `Track ${String(i + 1).padStart(2, '0')} — Demo Session`, duration: 150 + i * 13 })))
    Object.assign(d, { status: 'playlist', stage: null, title: 'Late Night Demo Sessions', playlist: { title: 'Late Night Demo Sessions', count: n, truncated: false, nested: 0 }, thumbnail: yt('5qap5aO4i9A') })
    return push(d)
  }
  if (/fail/.test(d.url)) {
    Object.assign(d, { status: 'error', stage: null, error_msg: 'This video is private.', error_detail: 'Private video. Sign in if you\'ve been granted access', finished_at: Date.now() })
    return push(d)
  }

  if (!d.title) {
    Object.assign(d, { title: 'Rick Astley - Never Gonna Give You Up (Official Video) (4K Remaster)', thumbnail: yt('dQw4w9WgXcQ'), duration: 213, site: 'Youtube' })
  }
  push(d)
  const two = d.format === 'video'
  const stages = two ? [['video', 0.85], ['audio', 0.15]] : [['downloading', 1]]
  let base = 0
  for (const [stage, weight] of stages) {
    for (let p = 0; p <= 1.0001; p += 0.04 + Math.random() * 0.03) {
      if (!alive()) return finishCancel(d)
      Object.assign(d, { status: 'downloading', stage, progress: Math.min(100, (base + p * weight) * 100), speed: 2.4e6 + Math.random() * 3e6, eta: Math.max(1, (1 - p) * 12) })
      push(d)
      await sleep(220)
    }
    base += weight
  }
  Object.assign(d, { status: 'processing', stage: two ? 'merging' : 'converting', speed: null, eta: null }); push(d)
  await sleep(1300)
  if (!alive()) return finishCancel(d)
  Object.assign(d, { status: 'done', stage: null, progress: 100, size_bytes: two ? 48_200_000 : 7_100_000, file_path: `${d.save_path}\\${d.title}.${d.ext}`, finished_at: Date.now() })
  push(d)
}

function finishCancel(d) {
  if (!db.has(d.id)) return
  Object.assign(d, { status: 'cancelled', stage: null, speed: null, eta: null, finished_at: Date.now() })
  delete d._cancel
  push(d)
}

function cancel(id) {
  const d = db.get(id)
  if (!d) return
  const qi = queue.indexOf(id)
  if (qi >= 0) { queue.splice(qi, 1); finishCancel(d) } else d._cancel = true
}

mockIPC(async (cmd, args = {}) => {
  switch (cmd) {
    case 'get_app_info': return { platform: 'windows', version: '0.1.0', app_id: 'lynk' }
    case 'get_preferences': return { ...prefs }
    case 'set_preference': prefs[args.key] = args.value; if (args.key === 'max_concurrent') pump(); return null
    case 'get_default_save_path': return 'C:\\Users\\you\\Downloads\\Lynk'
    case 'read_clipboard': return clipboard
    case 'list_downloads': return [...db.values()].sort((a, b) => b.created_at - a.created_at)
    case 'enqueue_downloads': {
      const out = args.items.map((it, i) => {
        const d = { id: uid(), url: it.url, title: null, format: it.format, quality: it.quality, audio_format: it.audio_format, video_format: it.video_format, prefer_h264: it.prefer_h264, ext: it.format === 'audio' ? it.audio_format : it.video_format, save_path: it.save_path, status: 'queued', progress: 0, created_at: Date.now() + i }
        db.set(d.id, d); queue.push(d.id); push(d)
        return d
      })
      pump()
      return out
    }
    case 'cancel_download': cancel(args.id); return null
    case 'cancel_group': for (const d of db.values()) if (d.group_id === args.groupId) cancel(d.id); return null
    case 'retry_download': {
      const d = db.get(args.id)
      Object.assign(d, { status: 'queued', progress: 0, error_msg: null, file_missing: false, created_at: Date.now() })
      queue.push(d.id); push(d); pump()
      return d
    }
    case 'remove_download': { const d = db.get(args.id); if (d) { d._cancel = true; db.delete(args.id); const qi = queue.indexOf(args.id); if (qi >= 0) queue.splice(qi, 1) } emit('download:removed', args.id); return null }
    case 'clear_history': {
      const ids = [...db.values()].filter(d => ['done', 'error', 'cancelled'].includes(d.status)).map(d => d.id)
      ids.forEach(i => db.delete(i))
      return ids
    }
    case 'confirm_playlist': {
      const parent = db.get(args.id)
      const entries = playlists.get(args.id)
      playlists.delete(args.id); db.delete(args.id); emit('download:removed', args.id)
      entries.forEach((e, i) => {
        const d = { id: uid(), url: `${parent.url}#${e.index}`, title: e.title, duration: e.duration, site: 'Youtube', format: parent.format, quality: parent.quality, audio_format: parent.audio_format, video_format: parent.video_format, ext: parent.ext, save_path: `${parent.save_path}\\${parent.title}`, status: 'queued', progress: 0, created_at: Date.now() - i, group_id: parent.id, group_title: parent.title, group_index: e.index, group_count: entries.length, thumbnail: yt(['5qap5aO4i9A', 'jfKfPfyJRdk', 'lTRiuFIWV54'][i % 3]) }
        db.set(d.id, d); queue.push(d.id); push(d)
      })
      pump()
      return entries.length
    }
    case 'engine_status': return { ...engine }
    case 'engine_update':
      engine = { ...engine, updating: true, error: null, updated_to: null }; emit('engine:status', engine)
      setTimeout(() => { engine = { ...engine, updating: false, version: '2026.08.19', updated_to: '2026.08.19', last_check: Date.now() }; emit('engine:status', engine) }, 2200)
      return null
    case 'update_status': return { ...appUpdate }
    case 'check_update':
      appUpdate = { state: 'checking' }; emit('app:update', appUpdate)
      setTimeout(() => { appUpdate = { state: 'ready', version: '0.2.0', notes: 'Faster playlists' }; emit('app:update', appUpdate) }, 1500)
      return { ...appUpdate }
    case 'install_update': console.info('[mock] install update'); return null
    case 'get_autostart': return true
    case 'set_autostart': return null
    case 'open_path': case 'reveal_path': case 'open_dir':
      console.info(`[mock] ${cmd}`, args.path); return null
    case 'window_minimize': case 'window_hide': case 'app_quit': return null
    case 'plugin:dialog|open': return 'D:\\Media\\Lynk'
    default:
      console.warn('[mock] unhandled command', cmd, args)
      return null
  }
}, { shouldMockEvents: true })
