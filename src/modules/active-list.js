// Downloads shown on the main screen: everything started this session (and
// anything still running), until the user dismisses it. History keeps all.
import { store } from './store.js'
import { EVENTS } from './events.js'
import { t } from './i18n.js'
import {
  createRow, updateRow, createPromptRow, updatePromptRow, createGroupRow, updateGroupRow,
  isActive, isFinished,
} from './card.js'

const records = new Map()
const session = new Set()   // record ids shown here
const nodes = new Map()     // entry key → element ('g:<group>' for playlists)
const expanded = new Set()  // expanded group keys
let listEl, mainEl, summaryEl, clearBtn
let renderQueued = false

const shows = d => isActive(d) || d.status === 'playlist'
const keyOf = d => (d.group_id ? `g:${d.group_id}` : d.id)

export function init() {
  listEl = document.getElementById('active-list')
  mainEl = document.getElementById('main')
  summaryEl = document.getElementById('activity-summary')
  clearBtn = document.getElementById('btn-clear-finished')

  store.on(EVENTS.DOWNLOADS_LOADED, list => {
    for (const d of list) {
      records.set(d.id, d)
      if (shows(d)) session.add(d.id)
    }
    scheduleRender()
  })
  store.on(EVENTS.DOWNLOAD_UPDATED, onUpdate)
  store.on(EVENTS.DOWNLOAD_REMOVED, id => {
    records.delete(id)
    if (session.delete(id)) scheduleRender()
  })
  store.on(EVENTS.DOWNLOAD_FOCUS, focus)

  listEl.addEventListener('click', onClick)
  listEl.addEventListener('dblclick', e => {
    const row = e.target.closest('.row[data-id]')
    if (!row || e.target.closest('button')) return
    const d = records.get(row.dataset.id)
    if (d?.status === 'done' && !d.file_missing) store.emit(EVENTS.DOWNLOAD_ACTION, { action: 'open', id: d.id })
  })
  clearBtn.addEventListener('click', clearFinished)
}

function onUpdate(d) {
  const prev = records.get(d.id)
  records.set(d.id, d)
  if (!session.has(d.id)) {
    if (!shows(d)) return
    session.add(d.id)
    return scheduleRender()
  }
  const key = keyOf(d)
  const node = nodes.get(key)
  const shapeChanged = (prev?.status === 'playlist') !== (d.status === 'playlist')
  if (!node || shapeChanged || renderQueued) return scheduleRender()

  if (d.group_id) updateGroup(key, d.id)
  else if (d.status === 'playlist') updatePromptRow(node, d)
  else updateRow(node, d)
  updateHead()
}

function scheduleRender() {
  if (renderQueued) return
  renderQueued = true
  requestAnimationFrame(() => { renderQueued = false; render() })
}

function entries() {
  const groups = new Map()
  for (const id of session) {
    const d = records.get(id)
    if (!d) { session.delete(id); continue }
    const key = keyOf(d)
    if (!groups.has(key)) groups.set(key, [])
    groups.get(key).push(d)
  }
  return [...groups.entries()]
    .map(([key, items]) => ({ key, items, time: Math.max(...items.map(i => i.created_at)) }))
    .sort((a, b) => b.time - a.time)
}

function render() {
  const list = entries()
  const wanted = new Set(list.map(e => e.key))
  for (const [key, node] of nodes) {
    if (!wanted.has(key)) { node.remove(); nodes.delete(key) }
  }

  let prev = null
  for (const { key, items } of list) {
    const first = items[0]
    const shape = key.startsWith('g:') ? 'group' : first.status === 'playlist' ? 'prompt' : 'row'
    let node = nodes.get(key)
    if (node && node._shape !== shape) { node.remove(); node = null }
    if (!node) {
      node = shape === 'group' ? createGroupRow(first.group_id, 'active')
        : shape === 'prompt' ? createPromptRow(first)
        : createRow(first, 'active')
      node._shape = shape
      nodes.set(key, node)
    }
    if (shape === 'group') updateGroup(key)
    else if (shape === 'prompt') updatePromptRow(node, first)
    else updateRow(node, first)

    const expectedAt = prev ? prev.nextSibling : listEl.firstChild
    if (node !== expectedAt) listEl.insertBefore(node, expectedAt)
    prev = node
  }

  mainEl.classList.toggle('has-items', list.length > 0)
  updateHead()
}

function groupChildren(key) {
  const gid = key.slice(2)
  return [...session]
    .map(id => records.get(id))
    .filter(d => d && d.group_id === gid)
    .sort((a, b) => (a.group_index ?? 0) - (b.group_index ?? 0))
}

function updateGroup(key, changedId = null) {
  const node = nodes.get(key)
  if (node) updateGroupRow(node, groupChildren(key), { expanded: expanded.has(key), changedId })
}

function updateHead() {
  let running = 0
  let waiting = 0
  let finished = 0
  for (const e of entries()) {
    for (const d of e.items) {
      if (d.status === 'queued') waiting++
      else if (isActive(d)) running++
    }
    if (e.items.every(isFinished)) finished++
  }
  summaryEl.textContent = t.summary(running, waiting)
  clearBtn.hidden = finished === 0
}

// ── Interaction ──────────────────────────────────────────────────────────────

function onClick(e) {
  const btn = e.target.closest('[data-action]')
  const row = e.target.closest('.row')
  if (!row) return
  const groupId = row.dataset.group
  // A click anywhere on a playlist row (or its arrow) expands it.
  if (groupId && (!btn || btn.dataset.action === 'toggle') && !e.target.closest('.group-children')) {
    const key = `g:${groupId}`
    expanded.has(key) ? expanded.delete(key) : expanded.add(key)
    return updateGroup(key)
  }
  if (!btn) return
  const action = btn.dataset.action
  const id = btn.closest('[data-id]')?.dataset.id
  if (action === 'dismiss') return dismiss([id], row)
  if (action === 'dismiss-group') {
    return dismiss([...session].filter(sid => records.get(sid)?.group_id === groupId), row)
  }
  store.emit(EVENTS.DOWNLOAD_ACTION, { action, id, groupId })
}

function dismiss(ids, row) {
  const apply = () => {
    ids.forEach(id => session.delete(id))
    render()
  }
  if (matchMedia('(prefers-reduced-motion: reduce)').matches) return apply()
  row.classList.add('leaving')
  row.addEventListener('animationend', apply, { once: true })
}

function clearFinished() {
  for (const e of entries()) {
    if (e.items.every(isFinished)) e.items.forEach(d => session.delete(d.id))
  }
  render()
}

function focus(id) {
  const d = records.get(id)
  if (!d) return
  if (!session.has(id)) { session.add(id); render() }
  const node = nodes.get(keyOf(d))
  if (!node) return
  node.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  node.classList.remove('flash')
  void node.offsetWidth
  node.classList.add('flash')
}
