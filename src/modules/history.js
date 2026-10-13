// History panel: every download, newest first, grouped by day; playlists
// collapse into one row. Renders lazily (only while open).
import { store } from './store.js'
import { EVENTS } from './events.js'
import { t } from './i18n.js'
import { esc } from './dom.js'
import { dayLabel, siteName } from './format.js'
import { confirm } from './dialog.js'
import { createRow, updateRow, createGroupRow, updateGroupRow, isActive } from './card.js'

const PAGE = 150
const records = new Map()
const nodes = new Map()      // entry key → element
const expanded = new Set()
let listEl, countEl, clearBtn, searchWrap, searchInput
let open = false
let query = ''
let limit = PAGE
let renderQueued = false

const keyOf = d => (d.group_id ? `g:${d.group_id}` : d.id)
const byIndex = (a, b) => (a.group_index ?? 0) - (b.group_index ?? 0)

export function init() {
  listEl = document.getElementById('history-list')
  countEl = document.getElementById('history-count')
  clearBtn = document.getElementById('btn-clear-history')
  searchWrap = document.getElementById('history-search')
  searchInput = document.getElementById('history-query')

  store.on(EVENTS.DOWNLOADS_LOADED, list => {
    list.forEach(d => records.set(d.id, d))
    scheduleRender()
  })
  store.on(EVENTS.DOWNLOAD_UPDATED, d => {
    const isNew = !records.has(d.id)
    records.set(d.id, d)
    if (!open) return
    const key = keyOf(d)
    const node = nodes.get(key)
    if (isNew || !node || renderQueued) return scheduleRender()
    if (d.group_id) updateGroupRow(node, children(d.group_id), { expanded: expanded.has(key), changedId: d.id })
    else updateRow(node, d)
  })
  store.on(EVENTS.DOWNLOAD_REMOVED, id => {
    if (records.delete(id)) scheduleRender()
  })
  store.on(EVENTS.PANEL_OPENED, name => {
    if (name !== 'history') return
    open = true
    limit = PAGE
    render()
  })
  store.on(EVENTS.PANEL_CLOSED, name => {
    if (name !== 'history') return
    open = false
    if (query) { query = ''; searchInput.value = '' }
  })

  searchInput.addEventListener('input', () => {
    query = searchInput.value.trim().toLowerCase()
    limit = PAGE
    render()
  })
  searchInput.addEventListener('keydown', e => {
    if (e.key === 'Escape' && searchInput.value) {
      e.stopPropagation()
      searchInput.value = ''
      query = ''
      render()
    }
  })
  clearBtn.addEventListener('click', clearHistory)
  listEl.addEventListener('click', onClick)
  listEl.addEventListener('dblclick', e => {
    const row = e.target.closest('.row[data-id]')
    if (!row || e.target.closest('button')) return
    const d = records.get(row.dataset.id)
    if (d?.status === 'done' && !d.file_missing) store.emit(EVENTS.DOWNLOAD_ACTION, { action: 'open', id: d.id })
  })
}

function children(groupId) {
  return [...records.values()].filter(d => d.group_id === groupId).sort(byIndex)
}

function matches(d) {
  if (!query) return true
  return [d.title, d.url, d.group_title, siteName(d), d.error_msg]
    .some(v => v && String(v).toLowerCase().includes(query))
}

function entries() {
  const groups = new Map()
  for (const d of records.values()) {
    if (d.status === 'playlist') continue
    const key = keyOf(d)
    if (!groups.has(key)) groups.set(key, [])
    groups.get(key).push(d)
  }
  return [...groups.entries()]
    .filter(([, items]) => items.some(matches))
    .map(([key, items]) => ({ key, items, time: Math.max(...items.map(i => i.finished_at || i.created_at)) }))
    .sort((a, b) => b.time - a.time)
}

function scheduleRender() {
  if (!open || renderQueued) return
  renderQueued = true
  requestAnimationFrame(() => { renderQueued = false; if (open) render() })
}

function render() {
  const all = entries()
  const total = new Set([...records.values()].filter(d => d.status !== 'playlist').map(keyOf)).size
  countEl.textContent = total ? String(total) : ''
  clearBtn.hidden = ![...records.values()].some(d => !isActive(d) && d.status !== 'playlist')
  searchWrap.hidden = total === 0

  if (!all.length) {
    nodes.forEach(n => n.remove())
    nodes.clear()
    listEl.innerHTML = query
      ? `<div class="empty"><div class="empty-title">${esc(t.noMatches)}</div></div>`
      : `<div class="empty"><div class="empty-title">${esc(t.noHistory)}</div><div class="empty-sub">${esc(t.noHistorySub)}</div></div>`
    return
  }

  // Rebuild the sequence (day headings are cheap); reuse row nodes.
  const shown = all.slice(0, limit)
  const wanted = new Set(shown.map(e => e.key))
  for (const [key, node] of nodes) {
    if (!wanted.has(key)) { node.remove(); nodes.delete(key) }
  }
  const frag = document.createDocumentFragment()
  let day = null
  for (const { key, items, time } of shown) {
    const label = dayLabel(time)
    if (label !== day) {
      day = label
      const h = document.createElement('div')
      h.className = 'day'
      h.textContent = label
      frag.appendChild(h)
    }
    let node = nodes.get(key)
    if (key.startsWith('g:')) {
      if (!node) { node = createGroupRow(key.slice(2), 'history'); nodes.set(key, node) }
      updateGroupRow(node, items.sort(byIndex), { expanded: expanded.has(key) })
    } else if (!node) {
      node = createRow(items[0], 'history')
      nodes.set(key, node)
    } else {
      updateRow(node, items[0])
    }
    frag.appendChild(node)
  }
  if (all.length > shown.length) {
    const more = document.createElement('button')
    more.className = 'text-btn more'
    more.dataset.more = ''
    more.textContent = t.showMore(all.length - shown.length)
    frag.appendChild(more)
  }
  listEl.replaceChildren(frag)
}

function onClick(e) {
  if (e.target.closest('[data-more]')) {
    limit += PAGE
    return render()
  }
  const btn = e.target.closest('[data-action]')
  const row = e.target.closest('.row')
  if (!row) return
  const groupId = row.dataset.group
  if (groupId && (!btn || btn.dataset.action === 'toggle') && !e.target.closest('.group-children')) {
    const key = `g:${groupId}`
    expanded.has(key) ? expanded.delete(key) : expanded.add(key)
    updateGroupRow(nodes.get(key), children(groupId), { expanded: expanded.has(key) })
    return
  }
  if (!btn) return
  store.emit(EVENTS.DOWNLOAD_ACTION, {
    action: btn.dataset.action,
    id: btn.closest('[data-id]')?.dataset.id,
    groupId,
  })
}

async function clearHistory() {
  const ok = await confirm({
    title: t.clearHistoryTitle,
    message: t.clearHistoryBody,
    confirmLabel: t.clearHistoryConfirm,
    destructive: true,
  })
  if (ok) store.emit(EVENTS.DOWNLOAD_ACTION, { action: 'clear-history' })
}
