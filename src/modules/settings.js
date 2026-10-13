import * as api from './tauri-api.js'
import * as prefs from './prefs.js'
import { store } from './store.js'
import { EVENTS } from './events.js'
import { t } from './i18n.js'
import { esc, syncSegmented, setChecked } from './dom.js'
import { icon } from './icons.js'
import { relativeTime } from './format.js'
import { confirm } from './dialog.js'

let body
let platform = 'windows'
let version = ''
let engine = { version: null, updating: false, last_check: null, error: null, updated_to: null }
let autostart = false
let appUpdate = { state: 'idle' }

export function init(appInfo) {
  body = document.getElementById('settings-body')
  platform = appInfo.platform
  version = appInfo.version
  render()

  store.on(EVENTS.PREF_UPDATED, ({ key, value }) => syncControl(key, value))
  store.on(EVENTS.ENGINE_STATUS, s => {
    const justUpdated = s.updated_to && s.updated_to !== engine.updated_to
    engine = s
    renderEngine()
    if (justUpdated) store.emit(EVENTS.TOAST, { message: t.engineUpdatedToast(s.updated_to), kind: 'success' })
  })
  store.on(EVENTS.UPDATE_STATUS, s => { appUpdate = s; renderAppUpdate() })
  store.on(EVENTS.PANEL_OPENED, async name => {
    if (name !== 'settings') return
    renderEngine()
    requestAnimationFrame(() => body.querySelectorAll('.segmented').forEach(s => syncSegmented(s, false)))
    // The registry can change outside the app.
    if (platform === 'windows') {
      autostart = await api.getAutostart().catch(() => autostart)
      const box = body.querySelector('#pref-autostart')
      if (box) box.checked = autostart
    }
  })
}

// ── Markup ───────────────────────────────────────────────────────────────────

const select = (id, options, value) => `
  <span class="select">
    <select id="${id}">
      ${options.map(o => `<option value="${esc(o.value)}"${o.value === value ? ' selected' : ''}>${esc(o.label)}</option>`).join('')}
    </select>
    ${icon('chevron-down', 14)}
  </span>`

const toggle = (id, checked) => `
  <span class="switch">
    <input type="checkbox" id="${id}"${checked ? ' checked' : ''}>
    <span class="switch-track"></span>
  </span>`

const segmented = (id, options, value) => `
  <div class="segmented small" id="${id}" role="radiogroup">
    <span class="seg-indicator" aria-hidden="true"></span>
    ${options.map(o => `<button class="seg-btn" role="radio" data-value="${esc(o.value)}" aria-checked="${o.value === value}">${esc(o.label)}</button>`).join('')}
  </div>`

const text = (label, sub, extra = '') => `
  <div class="s-text">
    <div class="s-label">${esc(label)}</div>
    ${sub ? `<div class="s-sub"${extra}>${esc(sub)}</div>` : ''}
  </div>`

function render() {
  const cookieOptions = platform === 'macos'
    ? [...t.cookieBrowsers, { value: 'safari', label: 'Safari' }]
    : t.cookieBrowsers
  const short = items => items.map(({ value, label }) => ({ value, label }))
  const parallel = ['1', '2', '3', '4', '5'].map(v => ({ value: v, label: v }))

  body.innerHTML = `
    <section class="s-section">
      <div class="s-title">${t.sVideo}</div>
      <div class="s-row">${text(t.sFormat)}${segmented('pref-vformat', short(t.videoFormats), prefs.get('video_format', 'mp4'))}</div>
      <div class="s-row">${text(t.sQuality)}${select('pref-quality', t.qualityOptions, prefs.get('default_quality', 'best'))}</div>
      <label class="s-row" for="pref-h264">${text(t.sPreferH264, t.sPreferH264Sub)}${toggle('pref-h264', prefs.get('prefer_h264', 'true') !== 'false')}</label>
    </section>

    <section class="s-section">
      <div class="s-title">${t.sAudio}</div>
      <div class="s-row">${text(t.sFormat)}${segmented('pref-audio', short(t.audioOptions), prefs.get('audio_format', 'mp3'))}</div>
    </section>

    <section class="s-section">
      <div class="s-title">${t.sSaving}</div>
      <div class="s-row">
        <div class="s-text">
          <div class="s-label">${t.sFolder}</div>
          <div class="s-path" id="save-path-text"></div>
        </div>
        <button class="text-btn" id="btn-change-folder">${t.sChange}</button>
      </div>
      <label class="s-row" for="pref-ask">${text(t.sAskEveryTime)}${toggle('pref-ask', prefs.getBool('ask_save_path'))}</label>
      <div class="s-row">${text(t.sParallel)}${segmented('pref-parallel', parallel, prefs.get('max_concurrent', '3'))}</div>
    </section>

    <section class="s-section">
      <div class="s-title">${t.sSignIn}</div>
      <div class="s-row">${text(t.sCookies, t.sCookiesSub)}${select('pref-cookies', cookieOptions, prefs.get('cookies_browser', ''))}</div>
    </section>

    <section class="s-section">
      <div class="s-title">${t.sGeneral}</div>
      ${platform === 'windows'
        ? `<label class="s-row" for="pref-autostart">${text(t.sAutostart)}${toggle('pref-autostart', autostart)}</label>`
        : ''}
      <label class="s-row" for="pref-notify">${text(t.sNotifications, t.sNotificationsSub)}${toggle('pref-notify', prefs.get('notifications', 'true') !== 'false')}</label>
    </section>

    <section class="s-section">
      <div class="s-title">${t.sEngine}</div>
      <div class="s-row">
        <div class="s-text">
          <div class="s-label" id="engine-version"></div>
          <div class="s-sub" id="engine-status"></div>
        </div>
        <button class="text-btn" id="btn-engine-update"></button>
      </div>
      <label class="s-row" for="pref-autoupdate">${text(t.sAutoUpdate)}${toggle('pref-autoupdate', prefs.get('ytdlp_auto_update', 'true') !== 'false')}</label>
    </section>

    <section class="s-section">
      <div class="s-title">${t.sApp}</div>
      <div class="s-row">
        <div class="s-text">
          <div class="s-label">${esc(t.sAppVersion(version))}</div>
          <div class="s-sub" id="app-update-status"></div>
        </div>
        <button class="text-btn" id="btn-app-update"></button>
      </div>
    </section>

    <div class="s-foot">
      <span></span>
      <button class="text-btn danger" id="btn-settings-clear">${t.sClearHistory}</button>
    </div>
  `
  renderSavePath()
  renderEngine()
  renderAppUpdate()
  wire()
}

function renderAppUpdate() {
  const s = body.querySelector('#app-update-status')
  const b = body.querySelector('#btn-app-update')
  if (!s) return
  const u = appUpdate
  s.className = 's-sub'
  s.textContent = u.state === 'checking' ? t.updChecking
    : u.state === 'downloading' ? t.updDownloading(u.version)
    : u.state === 'ready' ? t.updReady(u.version)
    : u.state === 'current' ? t.updCurrent
    : u.state === 'error' ? u.error || ''
    : ''
  if (u.state === 'error') s.classList.add('error')
  if (u.state === 'ready') s.classList.add('ok')
  s.hidden = !s.textContent
  b.hidden = u.state === 'checking' || u.state === 'downloading'
  b.textContent = u.state === 'ready' ? t.updRestart : t.updCheck
  b.dataset.mode = u.state === 'ready' ? 'install' : 'check'
}

function renderSavePath() {
  const el = body.querySelector('#save-path-text')
  const p = prefs.get('save_path')
  // RTL keeps the end of a long path visible; <bdi> keeps "C:\" in order.
  el.innerHTML = `<bdi>${esc(p)}</bdi>`
  el.title = p
}

function renderEngine() {
  const v = body.querySelector('#engine-version')
  const s = body.querySelector('#engine-status')
  const b = body.querySelector('#btn-engine-update')
  if (!v) return
  v.textContent = t.sEngineVersion(engine.version)
  s.className = 's-sub'
  if (engine.updating) {
    s.textContent = t.sEngineChecking
  } else if (engine.error) {
    s.textContent = engine.error
    s.classList.add('error')
  } else if (engine.updated_to) {
    s.textContent = t.sEngineUpdated(engine.updated_to)
    s.classList.add('ok')
  } else {
    s.textContent = engine.last_check ? t.sEngineChecked(relativeTime(engine.last_check)) : t.sEngineNever
  }
  b.disabled = engine.updating
  b.textContent = engine.updating ? '' : t.sCheck
  b.hidden = engine.updating
}

// ── Behaviour ────────────────────────────────────────────────────────────────

function wire() {
  const on = (sel, ev, fn) => body.querySelector(sel)?.addEventListener(ev, fn)

  on('#pref-quality', 'change', e => prefs.set('default_quality', e.target.value))
  on('#pref-h264', 'change', e => prefs.set('prefer_h264', e.target.checked))
  on('#pref-ask', 'change', e => prefs.set('ask_save_path', e.target.checked))
  on('#pref-notify', 'change', e => prefs.set('notifications', e.target.checked))
  on('#pref-autoupdate', 'change', e => prefs.set('ytdlp_auto_update', e.target.checked))
  on('#pref-cookies', 'change', e => prefs.set('cookies_browser', e.target.value))

  for (const [id, key] of [['#pref-vformat', 'video_format'], ['#pref-audio', 'audio_format'], ['#pref-parallel', 'max_concurrent']]) {
    on(id, 'click', e => {
      const btn = e.target.closest('[role="radio"]')
      if (btn) prefs.set(key, btn.dataset.value)
    })
  }

  on('#pref-autostart', 'change', async e => {
    const want = e.target.checked
    try {
      await api.setAutostart(want)
      autostart = want
    } catch (err) {
      e.target.checked = !want
      store.emit(EVENTS.TOAST, { message: `Couldn't change launch at login: ${err}`, kind: 'error' })
    }
  })

  on('#btn-change-folder', 'click', async () => {
    const chosen = await api.pickFolder(prefs.get('save_path'))
    if (chosen) prefs.set('save_path', chosen)
  })

  on('#btn-engine-update', 'click', () => {
    engine = { ...engine, updating: true, error: null, updated_to: null }
    renderEngine()
    api.engineUpdate()
  })

  on('#btn-app-update', 'click', e => {
    store.emit(e.currentTarget.dataset.mode === 'install' ? EVENTS.UPDATE_INSTALL : EVENTS.UPDATE_CHECK)
  })

  on('#btn-settings-clear', 'click', async () => {
    const ok = await confirm({
      title: t.clearHistoryTitle,
      message: t.clearHistoryBody,
      confirmLabel: t.clearHistoryConfirm,
      destructive: true,
    })
    if (ok) store.emit(EVENTS.DOWNLOAD_ACTION, { action: 'clear-history' })
  })
}

/** Keeps controls in sync when a pref changes elsewhere (e.g. the format menu). */
function syncControl(key, value) {
  const q = sel => body.querySelector(sel)
  const segs = { video_format: '#pref-vformat', audio_format: '#pref-audio', max_concurrent: '#pref-parallel' }
  if (segs[key]) {
    const seg = q(segs[key])
    if (seg) { setChecked(seg, value); syncSegmented(seg) }
    return
  }
  switch (key) {
    case 'default_quality': if (q('#pref-quality')) q('#pref-quality').value = value; break
    case 'cookies_browser': if (q('#pref-cookies')) q('#pref-cookies').value = value; break
    case 'prefer_h264': if (q('#pref-h264')) q('#pref-h264').checked = value !== 'false'; break
    case 'ask_save_path': if (q('#pref-ask')) q('#pref-ask').checked = value === 'true'; break
    case 'notifications': if (q('#pref-notify')) q('#pref-notify').checked = value !== 'false'; break
    case 'ytdlp_auto_update': if (q('#pref-autoupdate')) q('#pref-autoupdate').checked = value !== 'false'; break
    case 'save_path': renderSavePath(); break
  }
}
