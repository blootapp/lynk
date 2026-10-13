// Link field, Video/Audio toggle, format menu, paste / drop handling.
import * as api from './tauri-api.js'
import * as prefs from './prefs.js'
import { store } from './store.js'
import { EVENTS } from './events.js'
import { t } from './i18n.js'
import { esc, syncSegmented, setChecked, isEditable } from './dom.js'
import { icon } from './icons.js'
import { qualityShort } from './format.js'
import { extractUrls, normalizeUrl } from './links.js'
import { openMenu, isMenuOpen } from './popover.js'

let input, field, pasteBtn, goBtn, formatToggle, formatBtn
let format = 'video'
let busy = false
/** Normalized clipboard links already seen, so each is offered only once. */
const offered = new Set()

export function init() {
  input = document.getElementById('url-input')
  field = document.getElementById('field')
  pasteBtn = document.getElementById('btn-paste')
  goBtn = document.getElementById('btn-go')
  formatToggle = document.getElementById('format-toggle')
  formatBtn = document.getElementById('format-btn')

  format = prefs.get('default_format', 'video') === 'audio' ? 'audio' : 'video'
  setChecked(formatToggle, format)
  requestAnimationFrame(() => syncSegmented(formatToggle, false))
  renderFormatBtn()
  syncInput()

  input.addEventListener('input', () => { field.classList.remove('invalid'); syncInput() })
  input.addEventListener('keydown', e => {
    if (e.key === 'Enter') { e.preventDefault(); submit() }
    else if (e.key === 'Escape' && input.value) { e.preventDefault(); e.stopPropagation(); clearInput() }
  })
  pasteBtn.addEventListener('click', pasteFromClipboard)
  goBtn.addEventListener('click', () => submit())

  formatToggle.addEventListener('click', e => {
    const btn = e.target.closest('[role="radio"]')
    if (btn) setFormat(btn.dataset.value)
  })
  formatToggle.addEventListener('keydown', e => {
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      e.preventDefault()
      setFormat(format === 'video' ? 'audio' : 'video')
      formatToggle.querySelector('[aria-checked="true"]').focus()
    }
  })
  formatBtn.addEventListener('click', openFormatMenu)

  store.on(EVENTS.SUBMIT_DONE, onSubmitDone)
  store.on(EVENTS.PREF_UPDATED, ({ key }) => {
    if (['default_quality', 'audio_format', 'video_format'].includes(key)) renderFormatBtn()
  })

  // Offer a freshly copied link when the window comes forward.
  window.addEventListener('focus', offerClipboard)
  document.addEventListener('visibilitychange', () => { if (!document.hidden) offerClipboard() })
  offerClipboard()

  document.addEventListener('keydown', onGlobalKey)
  initDrop()
}

// ── Field ────────────────────────────────────────────────────────────────────

function syncInput() {
  const hasText = input.value.trim().length > 0
  pasteBtn.hidden = hasText
  goBtn.classList.toggle('ready', hasText)
  const n = extractUrls(input.value).length
  const label = n > 1 ? t.downloadN(n) : t.download
  goBtn.title = `${label} (Enter)`
  goBtn.setAttribute('aria-label', label)
}

function clearInput() {
  input.value = ''
  field.classList.remove('invalid')
  syncInput()
  input.focus()
}

function setInput(text) {
  input.value = text
  field.classList.remove('invalid')
  syncInput()
}

function flagInvalid() {
  field.classList.remove('invalid')
  void field.offsetWidth // restart the shake
  field.classList.add('invalid')
}

function setBusy(on) {
  busy = on
  goBtn.setAttribute('aria-busy', String(on))
  goBtn.innerHTML = on ? '<span class="spinner" aria-hidden="true"></span>' : icon('arrow-down', 18)
}

// ── Format ───────────────────────────────────────────────────────────────────

function setFormat(value) {
  if (value !== 'video' && value !== 'audio') return
  format = value
  setChecked(formatToggle, value)
  syncSegmented(formatToggle)
  renderFormatBtn()
  prefs.set('default_format', value)
}

function renderFormatBtn() {
  const parts = format === 'audio'
    ? [prefs.get('audio_format', 'mp3').toUpperCase()]
    : [prefs.get('video_format', 'mp4').toUpperCase(), qualityShort(prefs.get('default_quality', 'best'))]
  formatBtn.innerHTML = `${parts.map(esc).join('<span class="sep">·</span>')}${icon('chevron-down', 14)}`
  formatBtn.setAttribute('aria-label', `Format: ${parts.join(', ')}`)
}

function openFormatMenu() {
  if (formatBtn.getAttribute('aria-expanded') === 'true') return
  if (format === 'audio') {
    openMenu(formatBtn, [{
      label: t.menuFormat,
      items: t.audioOptions,
      value: prefs.get('audio_format', 'mp3'),
      onSelect: v => prefs.set('audio_format', v),
    }])
    return
  }
  openMenu(formatBtn, [
    {
      label: t.menuFormat,
      items: t.videoFormats,
      value: prefs.get('video_format', 'mp4'),
      onSelect: v => prefs.set('video_format', v),
    },
    {
      label: t.menuQuality,
      items: t.qualityOptions,
      value: prefs.get('default_quality', 'best'),
      onSelect: v => prefs.set('default_quality', v),
    },
  ])
}

// ── Submit ───────────────────────────────────────────────────────────────────

async function submit(textOverride) {
  if (busy) return
  let text = textOverride ?? input.value.trim()
  if (!text) {
    // Empty field + Download = "download what I copied".
    const clip = await api.readClipboard().catch(() => null)
    if (clip && extractUrls(clip).length) {
      setInput(clip)
      text = clip
    } else {
      flagInvalid()
      input.focus()
      store.emit(EVENTS.TOAST, { message: t.pasteFirst })
      return
    }
  }
  setBusy(true)
  store.emit(EVENTS.SUBMIT_URLS, { text, format })
}

function onSubmitDone({ ok, reason }) {
  setBusy(false)
  if (ok) {
    for (const u of extractUrls(input.value)) offered.add(normalizeUrl(u))
    input.value = ''
    syncInput()
    input.blur()
  } else if (reason === 'invalid') {
    flagInvalid()
    input.focus()
  }
}

// ── Clipboard ────────────────────────────────────────────────────────────────

async function pasteFromClipboard() {
  const clip = await api.readClipboard().catch(() => null)
  const urls = extractUrls(clip)
  if (urls.length) {
    setInput(urls.join(' '))
    input.focus()
    input.select()
  } else {
    store.emit(EVENTS.TOAST, { message: t.clipboardEmpty })
    input.focus()
  }
}

async function offerClipboard() {
  if (input.value || busy || document.hidden) return
  const clip = await api.readClipboard().catch(() => null)
  const urls = extractUrls(clip)
  if (urls.length === 0 || urls.length > 50) return
  // Offer a link only the first time it's seen, even if the field was busy
  // then — otherwise an old copy pops back up later.
  const key = urls.map(normalizeUrl).join(' ')
  if (offered.has(key)) return
  offered.add(key)
  if (input.value || busy) return
  setInput(urls.join(' '))
}

// ── Keyboard & drop ──────────────────────────────────────────────────────────

function onGlobalKey(e) {
  const mod = e.ctrlKey || e.metaKey
  if (isMenuOpen() || document.querySelector('.modal-backdrop')) return
  if (mod && e.key.toLowerCase() === 'v' && !isEditable(e.target)) {
    e.preventDefault()
    pasteFromClipboard()
  } else if (mod && e.key === ',') {
    e.preventDefault()
    store.emit(EVENTS.PANEL_OPEN, 'settings')
  } else if (mod && e.key.toLowerCase() === 'h') {
    e.preventDefault()
    store.emit(EVENTS.PANEL_OPEN, 'history')
  } else if (mod && e.key.toLowerCase() === 'l') {
    e.preventDefault()
    input.focus()
    input.select()
  }
}

function initDrop() {
  let depth = 0
  let overlay = null
  const hasLink = e => [...(e.dataTransfer?.types || [])].some(x => x === 'text/uri-list' || x === 'text/plain')
  const hide = () => { depth = 0; overlay?.remove(); overlay = null }

  document.addEventListener('dragenter', e => {
    if (!hasLink(e)) return
    e.preventDefault()
    depth++
    if (!overlay) {
      overlay = document.createElement('div')
      overlay.className = 'drop-overlay'
      overlay.innerHTML = `${icon('arrow-down', 22)}<span>${t.dropToDownload}</span>`
      document.getElementById('overlay-root').appendChild(overlay)
    }
  })
  document.addEventListener('dragover', e => {
    if (!hasLink(e)) return
    e.preventDefault()
    e.dataTransfer.dropEffect = 'copy'
  })
  document.addEventListener('dragleave', () => { if (--depth <= 0) hide() })
  document.addEventListener('drop', e => {
    if (!hasLink(e)) return
    e.preventDefault()
    hide()
    const urls = extractUrls(e.dataTransfer.getData('text/uri-list') || e.dataTransfer.getData('text/plain'))
    if (!urls.length) {
      store.emit(EVENTS.TOAST, { message: t.invalidLink, kind: 'error' })
      return
    }
    setInput(urls.join(' '))
    submit()
  })
}
