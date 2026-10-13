import { store } from './store.js'
import { EVENTS } from './events.js'
import { esc } from './dom.js'
import { icon } from './icons.js'

const ICONS = { info: null, success: 'check', error: 'circle-alert' }
let current = null
let timer = null

export function init() {
  store.on(EVENTS.TOAST, show)
}

function show({ message, kind = 'info', duration }) {
  const root = document.getElementById('toast-root')
  if (!root || !message) return
  clearTimeout(timer)
  current?.remove()

  const node = document.createElement('div')
  node.className = `toast ${kind}`
  node.innerHTML = `${ICONS[kind] ? icon(ICONS[kind], 15) : ''}<span>${esc(message)}</span>`
  root.appendChild(node)
  current = node

  timer = setTimeout(() => {
    node.classList.add('out')
    node.addEventListener('animationend', () => node.remove(), { once: true })
    if (current === node) current = null
  }, duration ?? (kind === 'error' ? 4200 : 2400))
}
