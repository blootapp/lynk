// History (left) and Settings (right) slide-over panels.
import { store } from './store.js'
import { EVENTS } from './events.js'

const panels = {}
let current = null
let opener = null

export function init() {
  panels.history = document.getElementById('history-panel')
  panels.settings = document.getElementById('settings-panel')

  document.getElementById('btn-history').addEventListener('click', () => toggle('history'))
  document.getElementById('btn-settings').addEventListener('click', () => toggle('settings'))
  document.querySelectorAll('[data-close-panel]').forEach(b => b.addEventListener('click', close))

  store.on(EVENTS.PANEL_OPEN, open)
}

export function open(name) {
  const panel = panels[name]
  if (!panel || current === name) return
  if (current) close({ silent: true })
  opener = document.activeElement
  panel.inert = false
  panel.classList.add('open')
  current = name
  store.emit(EVENTS.PANEL_OPENED, name)
  // Focus the panel itself: keyboard users Tab from there, and nothing
  // draws a focus ring just because the panel opened.
  requestAnimationFrame(() => panel.focus({ preventScroll: true }))
}

export function close({ silent = false } = {}) {
  if (!current) return
  const panel = panels[current]
  const name = current
  panel.classList.remove('open')
  panel.inert = true
  current = null
  store.emit(EVENTS.PANEL_CLOSED, name)
  if (!silent && opener instanceof HTMLElement && document.contains(opener)) opener.focus({ preventScroll: true })
  opener = null
}

export function toggle(name) {
  current === name ? close() : open(name)
}

export function currentPanel() {
  return current
}
