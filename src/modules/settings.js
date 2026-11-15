// Settings holds only what a person would change, in plain words: few
// settings with good defaults (as Apple's HIG puts it). Format and quality
// are picked next to the link field, and lynk decides the rest itself: the
// video codec (parse::video_sort), how many downloads run at once, and
// keeping its downloader up to date. Up to 0.1.4 those were settings too.
import * as api from './tauri-api.js'
import * as prefs from './prefs.js'
import { store } from './store.js'
import { EVENTS } from './events.js'
import { t } from './i18n.js'
import { esc } from './dom.js'
import { icon } from './icons.js'
import { confirm } from './dialog.js'
import { openMenu, closeMenu } from './popover.js'
import { browserOptions } from './signin.js'

let body
/** The dropdowns, by id: the pref each one sets and the choices it offers. */
let selects = {}
let platform = 'windows'
let version = ''
let engineVersion = null
let autostart = false
let appUpdate = { state: 'idle' }

export function init(appInfo) {
  body = document.getElementById('settings-body')
  platform = appInfo.platform
  version = appInfo.version
  render()

  store.on(EVENTS.PREF_UPDATED, ({ key, value }) => syncControl(key, value))
  // The downloader updates itself; its version shows only on hover, for support.
  store.on(EVENTS.ENGINE_STATUS, s => { engineVersion = s.version; renderVersion() })
  store.on(EVENTS.UPDATE_STATUS, s => { appUpdate = s; renderAppUpdate() })
  store.on(EVENTS.PANEL_OPENED, async name => {
    if (name !== 'settings') return
    // The registry can change outside the app.
    if (platform === 'windows') {
      autostart = await api.getAutostart().catch(() => autostart)
      const box = body.querySelector('#pref-autostart')
      if (box) box.checked = autostart
    }
  })
}

// ── Markup ───────────────────────────────────────────────────────────────────

// A dropdown is a button that opens lynk's own menu, like the format button
// next to the link field. A native <select> would open a list the system
// draws in its own style. Every choice sits in the same grid cell and only the
// current one shows, so the button is as wide as the longest choice and keeps
// its size when another one is picked.
const select = id => `
  <button class="select" id="${id}" type="button" aria-haspopup="menu" aria-expanded="false">
    <span class="select-value">
      ${selects[id].options.map(o => `<span data-value="${esc(o.value)}">${esc(o.label)}</span>`).join('')}
    </span>
    ${icon('chevron-down', 14)}
  </button>`

const toggle = (id, checked) => `
  <span class="switch">
    <input type="checkbox" id="${id}"${checked ? ' checked' : ''}>
    <span class="switch-track"></span>
  </span>`

const text = (label, sub) => `
  <div class="s-text">
    <div class="s-label">${esc(label)}</div>
    ${sub ? `<div class="s-sub">${esc(sub)}</div>` : ''}
  </div>`

function render() {
  selects = {
    'pref-cookies': { key: 'cookies_browser', fallback: '', label: t.sSignInFrom, options: browserOptions() },
  }

  body.innerHTML = `
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
    </section>

    <section class="s-section">
      <div class="s-title">${t.sSignIn}</div>
      <div class="s-row">${text(t.sSignInFrom, t.sSignInSub)}${select('pref-cookies')}</div>
    </section>

    <section class="s-section">
      <div class="s-title">${t.sGeneral}</div>
      ${platform === 'windows'
        ? `<label class="s-row" for="pref-autostart">${text(t.sAutostart)}${toggle('pref-autostart', autostart)}</label>`
        : ''}
      <label class="s-row" for="pref-notify">${text(t.sNotifications, t.sNotificationsSub)}${toggle('pref-notify', prefs.get('notifications', 'true') !== 'false')}</label>
    </section>

    <section class="s-section">
      <div class="s-title">${t.sUpdates}</div>
      <div class="s-row">
        <div class="s-text">
          <div class="s-label" id="app-version">${esc(t.sAppVersion(version))}</div>
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
  for (const [id, s] of Object.entries(selects)) setSelect(id, prefs.get(s.key, s.fallback))
  renderSavePath()
  renderVersion()
  renderAppUpdate()
  wire()
}

/** Shows `value` as the dropdown's current choice. */
function setSelect(id, value) {
  const btn = body.querySelector(`#${id}`)
  if (!btn) return
  btn.dataset.value = value
  let label = ''
  for (const choice of btn.querySelectorAll('.select-value > span')) {
    const on = choice.dataset.value === value
    choice.classList.toggle('on', on)
    if (on) label = choice.textContent
  }
  btn.setAttribute('aria-label', `${selects[id].label}: ${label}`)
}

function renderAppUpdate() {
  const s = body.querySelector('#app-update-status')
  const b = body.querySelector('#btn-app-update')
  if (!s) return
  const u = appUpdate
  s.className = 's-sub'
  // Always one line of text, so the row never changes height.
  s.textContent = u.state === 'checking' ? t.updChecking
    : u.state === 'downloading' ? t.updDownloading(u.version)
    : u.state === 'ready' ? t.updReady(u.version)
    : u.state === 'current' ? t.updCurrent
    : u.state === 'error' && u.error ? u.error
    : t.updAuto
  if (u.state === 'error' && u.error) s.classList.add('error')
  if (u.state === 'ready') s.classList.add('ok')
  b.hidden = u.state === 'checking' || u.state === 'downloading'
  b.textContent = u.state === 'ready' ? t.updRestart : t.updCheck
  b.dataset.mode = u.state === 'ready' ? 'install' : 'check'
}

function renderVersion() {
  const v = body.querySelector('#app-version')
  if (v) v.title = engineVersion ? t.sDownloaderVersion(engineVersion) : ''
}

function renderSavePath() {
  const el = body.querySelector('#save-path-text')
  const p = prefs.get('save_path')
  // RTL keeps the end of a long path visible; <bdi> keeps "C:\" in order.
  el.innerHTML = `<bdi>${esc(p)}</bdi>`
  el.title = p
}

// ── Behaviour ────────────────────────────────────────────────────────────────

function wire() {
  const on = (sel, ev, fn) => body.querySelector(sel)?.addEventListener(ev, fn)

  on('#pref-ask', 'change', e => prefs.set('ask_save_path', e.target.checked))
  on('#pref-notify', 'change', e => prefs.set('notifications', e.target.checked))

  for (const [id, s] of Object.entries(selects)) {
    const btn = body.querySelector(`#${id}`)
    const isOpen = () => btn.getAttribute('aria-expanded') === 'true'
    const open = () => openMenu(btn, [{
      items: s.options,
      value: btn.dataset.value,
      onSelect: v => prefs.set(s.key, v),
    }])
    btn.addEventListener('click', () => (isOpen() ? closeMenu() : open()))
    // As on a native dropdown, the arrow keys open it too.
    btn.addEventListener('keydown', e => {
      if ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && !isOpen()) {
        e.preventDefault()
        open()
      }
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

  on('#btn-app-update', 'click', e => {
    if (e.currentTarget.dataset.mode === 'install') return store.emit(EVENTS.UPDATE_INSTALL)
    store.emit(EVENTS.UPDATE_CHECK)
    // One button for both lynk and the downloader it runs.
    api.engineUpdate().catch(() => {})
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

/** Keeps controls in sync when a pref changes elsewhere (e.g. a row's sign-in button). */
function syncControl(key, value) {
  const q = sel => body.querySelector(sel)
  switch (key) {
    case 'cookies_browser': setSelect('pref-cookies', value); break
    case 'ask_save_path': if (q('#pref-ask')) q('#pref-ask').checked = value === 'true'; break
    case 'notifications': if (q('#pref-notify')) q('#pref-notify').checked = value !== 'false'; break
    case 'save_path': renderSavePath(); break
  }
}
