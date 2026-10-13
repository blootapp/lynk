// Download rows, shared by the main screen and history. Rows are built once
// and patched in place on every update, so the progress bar animates
// smoothly and thumbnails never reload.
import { t } from './i18n.js'
import { esc, el } from './dom.js'
import { icon } from './icons.js'
import { formatBytes, formatSpeed, formatEta, siteName, formatLabel, timeOfDay, hostOf } from './format.js'

const ACTIVE = new Set(['queued', 'fetching', 'downloading', 'processing'])

export const isActive = d => ACTIVE.has(d.status)
export const isFinished = d => d.status === 'done' || d.status === 'error' || d.status === 'cancelled'

const ROW = `
  <div class="thumb"><span data-r="glyph"></span><img data-r="img" alt="" referrerpolicy="no-referrer" decoding="async" draggable="false"></div>
  <div class="row-main">
    <div class="row-title" data-r="title"></div>
    <div class="row-meta" data-r="meta"></div>
    <div class="bar" data-r="bar" hidden><div data-r="fill"></div></div>
  </div>`

// ── Single download ──────────────────────────────────────────────────────────

/** context: 'active' (main screen) | 'history' */
export function createRow(d, context) {
  const node = el(`<article class="row" data-id="${esc(d.id)}">${ROW}<div class="row-actions" data-r="actions"></div></article>`)
  node._ctx = context
  node._r = refs(node)
  wireThumb(node._r.img)
  updateRow(node, d)
  return node
}

export function updateRow(node, d) {
  const r = node._r
  node.dataset.status = d.status
  node.classList.toggle('missing', d.status === 'done' && !!d.file_missing)

  setThumb(node, d.thumbnail, d.format === 'audio' ? 'music' : 'film')
  const title = d.title || prettyUrl(d.url)
  setText(r.title, title)
  r.title.title = title
  r.title.classList.toggle('placeholder', !d.title)

  const m = describe(d, node._ctx)
  setHTML(r.meta, `${m.icon ? icon(m.icon, 13) : ''}<span>${esc(m.text)}</span>`)
  r.meta.title = m.title || ''
  setBar(r, m.bar, d.progress)

  setActions(node, actionsFor(d, node._ctx))
  r.actions.classList.toggle('pinned', d.status === 'error')
}

function describe(d, ctx) {
  const when = ctx === 'history' ? timeOfDay(d.finished_at || d.created_at) : ''
  switch (d.status) {
    case 'queued':
      return { text: t.waiting }
    case 'fetching':
      return {
        bar: 'indeterminate',
        text: d.stage === 'engine' ? t.updatingEngine
          : d.stage === 'listing' ? t.readingPlaylist(d.playlist?.count)
          : t.fetching,
      }
    case 'downloading': {
      const parts = [`${Math.floor(d.progress || 0)}%`, formatSpeed(d.speed)]
      if (ctx === 'active') parts.push(formatEta(d.eta))
      return { bar: 'progress', text: join(...parts) }
    }
    case 'processing': {
      const fmt = (d.format === 'audio' ? d.audio_format : d.video_format || 'mp4').toUpperCase()
      return {
        bar: 'working',
        text: d.stage === 'merging' ? t.merging : d.stage === 'converting' ? t.converting(fmt) : t.finishing,
      }
    }
    case 'done':
      if (d.file_missing) return { text: join(t.fileMissing, when) }
      return { icon: 'check', text: join(siteName(d), formatLabel(d), formatBytes(d.size_bytes), when) }
    case 'error':
      return { icon: 'circle-alert', text: d.error_msg || t.failed, title: d.error_detail }
    case 'cancelled':
      return { text: join(t.cancelled, when) }
    default:
      return { text: '' }
  }
}

function actionsFor(d, ctx) {
  const close = ctx === 'active'
    ? { action: 'dismiss', icon: 'x', label: t.dismiss }
    : { action: 'remove', icon: 'trash-2', label: t.remove, danger: true }
  if (isActive(d)) return [{ action: 'cancel', icon: 'x', label: t.cancel, danger: true }]
  if (d.status === 'done' && !d.file_missing) {
    return [
      { action: 'open', icon: 'play', label: t.open },
      { action: 'reveal', icon: 'folder-open', label: t.showInFolder },
      close,
    ]
  }
  if (isFinished(d)) {
    return [{ action: 'retry', icon: 'rotate-cw', label: d.status === 'done' ? t.redownload : t.retry }, close]
  }
  return []
}

// ── Playlist prompt ──────────────────────────────────────────────────────────

export function createPromptRow(d) {
  const node = el(`
    <article class="row prompt" data-id="${esc(d.id)}" data-status="playlist">
      ${ROW}
      <div></div>
      <div class="prompt-actions">
        <button class="btn btn-primary" data-action="confirm-playlist" data-r="confirm"></button>
        <button class="btn btn-quiet" data-action="dismiss-playlist">${esc(t.notNow)}</button>
      </div>
    </article>`)
  node._r = refs(node)
  wireThumb(node._r.img)
  updatePromptRow(node, d)
  return node
}

export function updatePromptRow(node, d) {
  const r = node._r
  const p = d.playlist || { title: d.title, count: 0 }
  setThumb(node, d.thumbnail, 'list-video')
  setText(r.title, p.title || d.title || t.playlist)
  r.title.title = r.title.textContent
  const count = p.truncated ? `${t.items(p.count)} (${t.firstN(p.count)})` : t.items(p.count)
  setHTML(r.meta, `<span>${esc(join(t.playlist, count, formatLabel(d)))}</span>`)
  setText(r.confirm, t.downloadAll(p.count))
}

// ── Playlist group ───────────────────────────────────────────────────────────

/** context: 'active' | 'history' */
export function createGroupRow(groupId, context) {
  const node = el(`
    <article class="row group" data-group="${esc(groupId)}">
      ${ROW}
      <div class="row-actions pinned" data-r="actions"></div>
      <div class="group-children" data-r="children" hidden></div>
    </article>`)
  node.querySelector('.thumb').insertAdjacentHTML('beforeend', '<span class="thumb-count" data-r="count"></span>')
  node._ctx = context
  node._r = refs(node)
  node._children = new Map()
  wireThumb(node._r.img)
  return node
}

/**
 * `children` sorted by group_index. Pass `changedId` to patch only that
 * item's line (cheap); omit it to rebuild the expanded list.
 */
export function updateGroupRow(node, children, { expanded = false, changedId = null } = {}) {
  const r = node._r
  const total = children.length
  let done = 0, failed = 0, cancelled = 0, active = 0, running = 0, sum = 0, bytes = 0, speed = 0
  for (const c of children) {
    if (c.status === 'done') { done++; sum += 100; bytes += c.size_bytes || 0 }
    else if (c.status === 'error') failed++
    else if (c.status === 'cancelled') cancelled++
    else {
      active++
      sum += c.progress || 0
      if (c.status !== 'queued') running++
      if (c.status === 'downloading') speed += c.speed || 0
    }
  }
  const first = children[0] || {}
  node.dataset.status = active ? 'downloading' : failed ? 'error' : cancelled ? 'cancelled' : 'done'
  node.classList.toggle('expanded', expanded)

  setThumb(node, children.find(c => c.thumbnail)?.thumbnail, 'list-video')
  setText(r.count, String(total))
  setText(r.title, first.group_title || t.playlist)
  r.title.title = r.title.textContent

  let meta
  if (active) meta = { text: join(t.groupProgress(done, total), running ? t.groupRunning(running) : t.waiting, formatSpeed(speed)) }
  else if (failed || cancelled) meta = { icon: failed ? 'circle-alert' : null, text: t.groupPartial(done, total, failed, cancelled) }
  else meta = { icon: 'check', text: join(t.items(total), formatLabel(first), formatBytes(bytes)) }
  if (node._ctx === 'history') meta.text = join(meta.text, timeOfDay(Math.max(...children.map(c => c.finished_at || c.created_at))))
  setHTML(r.meta, `${meta.icon ? icon(meta.icon, 13) : ''}<span>${esc(meta.text)}</span>`)
  setBar(r, active ? 'progress' : null, sum / Math.max(total, 1))

  const acts = [{ action: 'toggle', icon: 'chevron-down', label: expanded ? t.collapse : t.expand, cls: 'toggle' }]
  if (active) acts.push({ action: 'cancel-group', icon: 'x', label: t.cancelAll, danger: true })
  else {
    if (failed || cancelled) acts.push({ action: 'retry-group', icon: 'rotate-cw', label: t.retryRest })
    if (done) acts.push({ action: 'open-group-folder', icon: 'folder-open', label: t.openFolder })
    acts.push(node._ctx === 'active'
      ? { action: 'dismiss-group', icon: 'x', label: t.dismiss }
      : { action: 'remove-group', icon: 'trash-2', label: t.remove, danger: true })
  }
  setActions(node, acts)

  r.children.hidden = !expanded
  if (!expanded) return
  if (changedId && node._children.has(changedId)) {
    const c = children.find(x => x.id === changedId)
    if (c) updateChild(node._children.get(changedId), c)
    return
  }
  const wanted = new Set(children.map(c => c.id))
  for (const [id, line] of node._children) {
    if (!wanted.has(id)) { line.remove(); node._children.delete(id) }
  }
  for (const c of children) {
    let line = node._children.get(c.id)
    if (!line) {
      line = el(`<div class="child" data-id="${esc(c.id)}"><span class="child-index"></span><span class="child-title"></span><span class="child-state"></span><span class="child-act"></span></div>`)
      node._children.set(c.id, line)
    }
    updateChild(line, c)
    r.children.appendChild(line)
  }
}

function updateChild(line, c) {
  const [index, title, state, act] = line.children
  line.dataset.status = c.status
  setText(index, String(c.group_index ?? '').padStart(2, '0'))
  setText(title, c.title || prettyUrl(c.url))
  title.title = title.textContent
  const s = c.status === 'downloading' ? `${Math.floor(c.progress || 0)}%`
    : c.status === 'processing' ? t.processingShort
    : c.status === 'fetching' ? t.starting
    : c.status === 'queued' ? t.waiting
    : c.status === 'done' ? (c.file_missing ? t.missingShort : formatBytes(c.size_bytes) || t.done)
    : c.status === 'error' ? t.failed
    : t.cancelled
  setText(state, s)
  state.title = c.status === 'error' ? c.error_msg || '' : ''
  const a = isActive(c) ? { action: 'cancel', icon: 'x', label: t.cancel }
    : c.status === 'error' || c.status === 'cancelled' ? { action: 'retry', icon: 'rotate-cw', label: t.retry }
    : c.status === 'done' && !c.file_missing ? { action: 'open', icon: 'play', label: t.open }
    : null
  const key = a?.action || ''
  if (act._key !== key) {
    act._key = key
    act.innerHTML = a ? button(a, 14) : ''
  }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

function refs(node) {
  const out = {}
  node.querySelectorAll('[data-r]').forEach(e => { out[e.dataset.r] = e })
  return out
}

function wireThumb(img) {
  img.addEventListener('load', () => img.classList.add('loaded'))
  img.addEventListener('error', () => img.classList.remove('loaded'))
}

function setThumb(node, url, glyph) {
  const r = node._r
  if (node._glyph !== glyph) {
    node._glyph = glyph
    r.glyph.innerHTML = icon(glyph, 14)
  }
  if (url && node._thumb !== url) {
    node._thumb = url
    r.img.classList.remove('loaded')
    r.img.src = url
  }
}

function setBar(r, mode, progress) {
  r.bar.hidden = !mode
  if (!mode) return
  const cls = `bar${mode === 'indeterminate' ? ' indeterminate' : mode === 'working' ? ' working' : ''}`
  if (r.bar.className !== cls) r.bar.className = cls
  r.fill.style.transform = mode === 'progress'
    ? `scaleX(${Math.min(1, Math.max(0.01, (progress || 0) / 100))})`
    : ''
}

function button(a, size = 16) {
  return `<button class="icon-btn${a.danger ? ' danger' : ''}${a.cls ? ` ${a.cls}` : ''}" data-action="${a.action}"
    title="${esc(a.label)}" aria-label="${esc(a.label)}">${icon(a.icon, size)}</button>`
}

function setActions(node, acts) {
  const r = node._r
  const key = acts.map(a => a.action).join(',')
  if (node._actsKey !== key) {
    node._actsKey = key
    r.actions.innerHTML = acts.map(a => button(a)).join('')
    return
  }
  // Same buttons; labels may still change (expand ↔ collapse).
  for (const a of acts) {
    const b = r.actions.querySelector(`[data-action="${a.action}"]`)
    if (b && b.title !== a.label) { b.title = a.label; b.setAttribute('aria-label', a.label) }
  }
}

function setText(e, value) {
  if (e.textContent !== value) e.textContent = value
}

function setHTML(e, html) {
  if (e._html !== html) { e._html = html; e.innerHTML = html }
}

const join = (...parts) => parts.filter(Boolean).join(' · ')

function prettyUrl(url) {
  try {
    const u = new URL(url)
    const path = decodeURIComponent(u.pathname + u.search).replace(/\/$/, '')
    return hostOf(url) + (path.length > 48 ? `${path.slice(0, 48)}…` : path)
  } catch {
    return url
  }
}
