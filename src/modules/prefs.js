// Shared preference values. Writes persist through the backend and are
// broadcast as PREF_UPDATED so every module stays in sync.
import * as api from './tauri-api.js'
import { store } from './store.js'
import { EVENTS } from './events.js'

let values = {}

export async function load() {
  values = await api.getPreferences()
}

export function get(key, fallback = '') {
  const v = values[key]
  return v === undefined || v === null ? fallback : v
}

export function getBool(key, fallback = false) {
  const v = values[key]
  return v === undefined ? fallback : v === 'true'
}

export async function set(key, value) {
  value = String(value)
  if (values[key] === value) return
  const previous = values[key]
  values[key] = value
  store.emit(EVENTS.PREF_UPDATED, { key, value })
  try {
    await api.setPreference(key, value)
  } catch (e) {
    values[key] = previous
    store.emit(EVENTS.PREF_UPDATED, { key, value: previous })
    store.emit(EVENTS.TOAST, { message: `Couldn't save setting: ${e}`, kind: 'error' })
  }
}
