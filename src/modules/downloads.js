// Download state and actions. Mirrors backend events into the store and
// turns user intents (submit, cancel, retry, …) into backend calls.
import * as api from './tauri-api.js'
import * as prefs from './prefs.js'
import { store } from './store.js'
import { EVENTS } from './events.js'
import { t } from './i18n.js'
import { extractUrls, normalizeUrl } from './links.js'

const records = new Map()
const IN_FLIGHT = new Set(['queued', 'fetching', 'downloading', 'processing', 'playlist'])

export async function init() {
  // Handle intents right away — a link submitted during startup must not be lost.
  store.on(EVENTS.SUBMIT_URLS, submit)
  store.on(EVENTS.DOWNLOAD_ACTION, act)

  // Listen before loading so nothing that happens in between is missed.
  await api.onDownloadUpdate(d => {
    records.set(d.id, d)
    store.emit(EVENTS.DOWNLOAD_UPDATED, d)
  })
  await api.onDownloadRemoved(id => {
    if (records.delete(id)) store.emit(EVENTS.DOWNLOAD_REMOVED, id)
  })

  const list = await api.listDownloads()
  for (const d of list) if (!records.has(d.id)) records.set(d.id, d)
  store.emit(EVENTS.DOWNLOADS_LOADED, [...records.values()])
}

// ── New downloads ────────────────────────────────────────────────────────────

async function submit({ text, format }) {
  const done = result => store.emit(EVENTS.SUBMIT_DONE, result)
  const urls = extractUrls(text)
  if (!urls.length) {
    toast(t.invalidLink, 'error')
    return done({ ok: false, reason: 'invalid' })
  }

  const inFlight = new Map()
  for (const r of records.values()) {
    if (IN_FLIGHT.has(r.status)) inFlight.set(`${normalizeUrl(r.url)}|${r.format}`, r.id)
  }
  const fresh = []
  const dupes = []
  for (const url of urls) {
    const key = `${normalizeUrl(url)}|${format}`
    if (inFlight.has(key)) dupes.push(inFlight.get(key))
    else { fresh.push(url); inFlight.set(key, null) }
  }
  if (!fresh.length) {
    toast(t.alreadyDownloading)
    if (dupes[0]) store.emit(EVENTS.DOWNLOAD_FOCUS, dupes[0])
    return done({ ok: true })
  }

  const savePath = await resolveSavePath()
  if (!savePath) return done({ ok: false, reason: 'cancelled' })

  try {
    const created = await api.enqueueDownloads(fresh.map(url => ({
      url,
      format,
      quality: prefs.get('default_quality', 'best'),
      audio_format: prefs.get('audio_format', 'mp3'),
      video_format: prefs.get('video_format', 'mp4') === 'mkv' ? 'mkv' : 'mp4',
      prefer_h264: prefs.get('prefer_h264', 'true') !== 'false',
      save_path: savePath,
    })))
    if (dupes.length) toast(t.someAlreadyDownloading(dupes.length))
    else if (created.length > 1) toast(t.addedN(created.length), 'success')
    done({ ok: true })
  } catch (e) {
    toast(errorText(e), 'error')
    done({ ok: false, reason: 'error' })
  }
}

async function resolveSavePath() {
  const current = prefs.get('save_path') || await api.getDefaultSavePath()
  if (prefs.getBool('ask_save_path')) return api.pickFolder(current)
  return current
}

// ── Actions ──────────────────────────────────────────────────────────────────

async function act({ action, id, groupId }) {
  const d = id ? records.get(id) : null
  const children = groupId ? [...records.values()].filter(r => r.group_id === groupId) : []
  try {
    switch (action) {
      case 'cancel':
        await api.cancelDownload(id)
        break
      case 'retry':
        await api.retryDownload(id)
        break
      case 'remove':
      case 'dismiss-playlist':
        await api.removeDownload(id)
        break
      case 'open':
        if (!d?.file_path) throw new Error(t.fileMissing)
        await api.openPath(d.file_path)
        break
      case 'reveal':
        await api.revealPath(d?.file_path || d?.save_path || prefs.get('save_path'))
        break
      case 'confirm-playlist': {
        const n = await api.confirmPlaylist(id)
        toast(t.addedPlaylist(n), 'success')
        break
      }
      case 'cancel-group':
        await api.cancelGroup(groupId)
        break
      case 'retry-group':
        for (const c of children) {
          if (c.status === 'error' || c.status === 'cancelled') await api.retryDownload(c.id)
        }
        break
      case 'open-group-folder': {
        const dir = children.find(c => c.save_path)?.save_path
        if (dir) await api.openDir(dir)
        break
      }
      case 'remove-group':
        for (const c of children) await api.removeDownload(c.id)
        break
      case 'clear-history': {
        const ids = await api.clearHistory()
        for (const rid of ids) {
          if (records.delete(rid)) store.emit(EVENTS.DOWNLOAD_REMOVED, rid)
        }
        toast(t.historyCleared, 'success')
        break
      }
    }
  } catch (e) {
    const msg = errorText(e)
    toast(msg, 'error')
    // Opening revealed the file is gone — reflect that on the card.
    if (d && action === 'open' && /moved or deleted/i.test(msg)) {
      const updated = { ...d, file_missing: true }
      records.set(d.id, updated)
      store.emit(EVENTS.DOWNLOAD_UPDATED, updated)
    }
  }
}

function toast(message, kind = 'info') {
  store.emit(EVENTS.TOAST, { message, kind })
}

function errorText(e) {
  return typeof e === 'string' ? e : e?.message || String(e)
}
