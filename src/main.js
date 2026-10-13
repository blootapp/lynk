import * as api from './modules/tauri-api.js'
import * as prefs from './modules/prefs.js'
import { store } from './modules/store.js'
import { EVENTS } from './modules/events.js'
import { hydrateIcons } from './modules/icons.js'
import { closeMenu, isMenuOpen } from './modules/popover.js'
import * as panels from './modules/panels.js'
import * as toast from './modules/toast.js'
import * as composer from './modules/composer.js'
import * as downloads from './modules/downloads.js'
import * as activeList from './modules/active-list.js'
import * as history from './modules/history.js'
import * as settings from './modules/settings.js'
import * as updater from './modules/updater.js'

async function main() {
  hydrateIcons()
  toast.init()

  // Browser preview (`npm run dev` outside Tauri) runs against a mock backend.
  if (import.meta.env.DEV && !window.__TAURI_INTERNALS__?.invoke) {
    await import('./dev/mock-tauri.js')
  }

  const appInfo = await api.getAppInfo()
  if (appInfo.platform === 'windows') {
    document.getElementById('title-bar').hidden = false
    document.documentElement.classList.add('platform-windows')
    document.getElementById('btn-minimize').addEventListener('click', api.windowMinimize)
    document.getElementById('btn-close').addEventListener('click', api.windowHide)
  } else {
    document.getElementById('mac-titlebar').hidden = false
    document.documentElement.classList.add('platform-macos')
    // Shortcut hints use the Mac modifier.
    document.querySelectorAll('[title*="Ctrl+"]').forEach(el => {
      el.title = el.title.replace(/Ctrl\+/g, '⌘')
    })
  }

  await prefs.load()

  panels.init()
  composer.init()
  activeList.init()
  history.init()
  settings.init(appInfo)
  initEscape()

  document.getElementById('btn-open-folder').addEventListener('click', () => {
    api.openDir(prefs.get('save_path')).catch(e =>
      store.emit(EVENTS.TOAST, { message: String(e), kind: 'error' }))
  })

  await downloads.init()

  await updater.init()
  await api.onEngineStatus(s => store.emit(EVENTS.ENGINE_STATUS, s))
  store.emit(EVENTS.ENGINE_STATUS, await api.engineStatus())
}

/** Escape closes the innermost thing: menu → panel. */
function initEscape() {
  document.addEventListener('keydown', e => {
    if (e.key !== 'Escape' || e.defaultPrevented) return
    if (isMenuOpen()) return closeMenu(true)
    if (panels.currentPanel()) panels.close()
  })
}

main().catch(err => {
  console.error(err)
  store.emit(EVENTS.TOAST, { message: `Lynk failed to start: ${err}`, kind: 'error', duration: 10000 })
})
