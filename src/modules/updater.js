// App self-update UI: the "Update ready · Restart" pill and the Settings row.
// The backend checks and downloads quietly; this only offers the restart.
import * as api from './tauri-api.js'
import { store } from './store.js'
import { EVENTS } from './events.js'
import { t } from './i18n.js'
import { confirm } from './dialog.js'
import { isActive } from './card.js'

const running = new Set()
let pill

export async function init() {
  pill = document.getElementById('update-pill')
  pill.addEventListener('click', restart)

  const track = d => (isActive(d) ? running.add(d.id) : running.delete(d.id))
  store.on(EVENTS.DOWNLOADS_LOADED, list => list.forEach(track))
  store.on(EVENTS.DOWNLOAD_UPDATED, track)
  store.on(EVENTS.DOWNLOAD_REMOVED, id => running.delete(id))
  store.on(EVENTS.UPDATE_CHECK, async () => apply(await api.checkUpdate()))
  store.on(EVENTS.UPDATE_INSTALL, restart)

  await api.onUpdateStatus(apply)
  apply(await api.updateStatus())
}

function apply(status) {
  if (!status) return
  pill.hidden = status.state !== 'ready'
  pill.title = status.version ? `Lynk ${status.version}${status.notes ? ` — ${status.notes}` : ''}` : ''
  store.emit(EVENTS.UPDATE_STATUS, status)
}

async function restart() {
  if (running.size) {
    const ok = await confirm({
      title: t.updRestartTitle,
      message: t.updRestartBody(running.size),
      confirmLabel: t.updRestart,
    })
    if (!ok) return
  }
  try {
    await api.installUpdate()
  } catch (e) {
    store.emit(EVENTS.TOAST, { message: String(e), kind: 'error' })
  }
}
