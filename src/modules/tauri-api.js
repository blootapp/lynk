import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { open as dialogOpen } from '@tauri-apps/plugin-dialog'

// ── App & window ─────────────────────────────────────────────────────────────

export const getAppInfo     = () => invoke('get_app_info')
export const windowMinimize = () => invoke('window_minimize')
export const windowHide     = () => invoke('window_hide')
export const appQuit        = () => invoke('app_quit')

// ── Preferences ──────────────────────────────────────────────────────────────

export const getPreferences = () => invoke('get_preferences')
export const setPreference  = (key, value) => invoke('set_preference', { key, value: String(value) })

// ── Files ────────────────────────────────────────────────────────────────────

export const getDefaultSavePath = () => invoke('get_default_save_path')
export const openPath   = path => invoke('open_path', { path })
export const revealPath = path => invoke('reveal_path', { path })
export const openDir    = path => invoke('open_dir', { path })
export const readClipboard = () => invoke('read_clipboard')

export async function pickFolder(defaultPath) {
  const chosen = await dialogOpen({ directory: true, multiple: false, defaultPath })
  return typeof chosen === 'string' ? chosen : null
}

// ── Downloads ────────────────────────────────────────────────────────────────

export const listDownloads    = () => invoke('list_downloads')
export const enqueueDownloads = items => invoke('enqueue_downloads', { items })
export const cancelDownload   = id => invoke('cancel_download', { id })
export const cancelGroup      = groupId => invoke('cancel_group', { groupId })
export const retryDownload    = id => invoke('retry_download', { id })
export const removeDownload   = id => invoke('remove_download', { id })
export const clearHistory     = () => invoke('clear_history')
export const confirmPlaylist  = id => invoke('confirm_playlist', { id })

// ── Engine & system ──────────────────────────────────────────────────────────

export const engineStatus = () => invoke('engine_status')
export const engineUpdate = () => invoke('engine_update')
export const getAutostart = () => invoke('get_autostart')
export const setAutostart = enabled => invoke('set_autostart', { enabled })

// ── Events (Rust → UI) ───────────────────────────────────────────────────────

export const onDownloadUpdate  = cb => listen('download:update', e => cb(e.payload))
export const onDownloadRemoved = cb => listen('download:removed', e => cb(e.payload))
export const onEngineStatus    = cb => listen('engine:status', e => cb(e.payload))
