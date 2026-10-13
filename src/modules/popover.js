import { esc } from './dom.js'
import { icon } from './icons.js'

let active = null

/**
 * Opens a menu of one or more single-choice sections under `anchor`.
 * sections: [{ label?, items: [{ value, label, hint? }], value, onSelect }]
 * With several sections the menu stays open so each can be set in turn.
 */
export function openMenu(anchor, sections) {
  closeMenu()
  const multi = sections.length > 1

  const menu = document.createElement('div')
  menu.className = 'menu'
  menu.setAttribute('role', 'menu')
  menu.innerHTML = sections.map((s, si) => `
    ${s.label ? `<div class="menu-label" role="presentation">${esc(s.label)}</div>` : ''}
    ${s.items.map(it => `
      <button class="menu-item" role="menuitemradio" data-section="${si}" data-value="${esc(it.value)}"
              aria-checked="${it.value === s.value}">
        <span class="check">${icon('check', 14)}</span>
        <span>${esc(it.label)}</span>
        ${it.hint ? `<span class="menu-hint">${esc(it.hint)}</span>` : ''}
      </button>`).join('')}`).join('')
  document.getElementById('overlay-root').appendChild(menu)

  // Right-align under the anchor; flip above when there's no room below.
  const r = anchor.getBoundingClientRect()
  const w = menu.offsetWidth
  const h = menu.offsetHeight
  const left = Math.max(8, Math.min(r.right - w, window.innerWidth - w - 8))
  let top = r.bottom + 6
  if (top + h > window.innerHeight - 8) top = Math.max(8, r.top - h - 6)
  menu.style.left = `${left}px`
  menu.style.top = `${top}px`
  if (top < r.top) menu.style.transformOrigin = 'bottom right'

  const buttons = [...menu.querySelectorAll('.menu-item')]
  const onKey = e => {
    const i = buttons.indexOf(document.activeElement)
    if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); closeMenu(true) }
    else if (e.key === 'ArrowDown') { e.preventDefault(); buttons[(i + 1) % buttons.length].focus() }
    else if (e.key === 'ArrowUp') { e.preventDefault(); buttons[(i - 1 + buttons.length) % buttons.length].focus() }
    else if (e.key === 'Tab') closeMenu()
  }
  const onOutside = e => { if (!menu.contains(e.target) && !anchor.contains(e.target)) closeMenu() }

  menu.addEventListener('click', e => {
    const btn = e.target.closest('.menu-item')
    if (!btn) return
    const section = sections[Number(btn.dataset.section)]
    menu.querySelectorAll(`.menu-item[data-section="${btn.dataset.section}"]`)
      .forEach(b => b.setAttribute('aria-checked', String(b === btn)))
    section.onSelect(btn.dataset.value)
    if (!multi) closeMenu(true)
  })
  document.addEventListener('keydown', onKey, true)
  document.addEventListener('mousedown', onOutside, true)
  window.addEventListener('blur', closeMenu)
  anchor.setAttribute('aria-expanded', 'true')

  active = { menu, anchor, cleanup: () => {
    document.removeEventListener('keydown', onKey, true)
    document.removeEventListener('mousedown', onOutside, true)
    window.removeEventListener('blur', closeMenu)
    anchor.setAttribute('aria-expanded', 'false')
  } }
  ;(buttons.find(b => b.getAttribute('aria-checked') === 'true') || buttons[0])?.focus({ preventScroll: true })
}

export function closeMenu(restoreFocus = false) {
  if (!active) return
  const { menu, anchor, cleanup } = active
  active = null
  cleanup()
  menu.remove()
  if (restoreFocus === true) anchor.focus({ preventScroll: true })
}

export function isMenuOpen() {
  return active !== null
}
