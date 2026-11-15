// "Use sign-in from": videos that need an account (age-restricted, private,
// members-only, YouTube's bot check) download with the sign-in of a browser
// on this computer (yt-dlp --cookies-from-browser). Settings shows the
// choice, and a failed download that sign-in would fix offers it right on
// its row, so nobody has to know the setting exists.
import * as api from './tauri-api.js'
import * as prefs from './prefs.js'
import { store } from './store.js'
import { EVENTS } from './events.js'
import { t } from './i18n.js'
import { openMenu, closeMenu } from './popover.js'

/** Browsers installed here, in lynk's order (system.rs). */
let installed = []

export async function load() {
  installed = await api.installedBrowsers().catch(() => [])
}

const option = id => ({ value: id, label: t.browserNames[id] || id })

/** Off, then this computer's browsers. A saved browser that's gone stays listed. */
export function browserOptions() {
  const saved = prefs.get('cookies_browser', '')
  const ids = saved && !installed.includes(saved) ? [...installed, saved] : installed
  return [{ value: '', label: t.signInOff }, ...ids.map(option)]
}

// The causes parse.rs::humanize_error answers with "use your browser's
// sign-in", matched on yt-dlp's own words, plus a browser whose sign-in
// couldn't be read (pick another one).
const CAUSES = [
  'not a bot', 'confirm you’re not', "confirm you're not",
  'confirm your age', 'age-restricted', 'age restricted', 'inappropriate for some users',
  'private video', 'video is private',
  'members-only', 'members only', 'join this channel',
  'login required', 'logged-in', 'log in', 'sign in', 'requires authentication', 'account credentials',
  'cookie', 'dpapi', 'app-bound', 'app bound',
]

/** A failed download that a browser's sign-in could fix. */
export function needsSignIn(d) {
  if (d.status !== 'error' || installed.length === 0) return false
  const text = `${d.error_detail || ''} ${d.error_msg || ''}`.toLowerCase()
  return CAUSES.some(c => text.includes(c))
}

/** The row's sign-in button: pick a browser, and lynk tries the download again. */
export function openSignInMenu(anchor, id) {
  if (anchor.getAttribute('aria-expanded') === 'true') return closeMenu()
  openMenu(anchor, [{
    label: t.sSignInFrom,
    items: installed.map(option),
    value: prefs.get('cookies_browser', ''),
    onSelect: async v => {
      await prefs.set('cookies_browser', v)
      store.emit(EVENTS.DOWNLOAD_ACTION, { action: 'retry', id })
    },
  }])
}
