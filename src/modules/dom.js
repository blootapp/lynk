export function esc(value) {
  return String(value ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;')
}

export function el(html) {
  const tpl = document.createElement('template')
  tpl.innerHTML = html.trim()
  return tpl.content.firstElementChild
}

/** Positions a segmented control's sliding indicator under the checked item. */
export function syncSegmented(root, animate = true) {
  const indicator = root.querySelector('.seg-indicator')
  const active = root.querySelector('[aria-checked="true"]')
  if (!indicator || !active || !active.offsetWidth) return
  if (!animate) indicator.style.transition = 'none'
  indicator.style.width = `${active.offsetWidth}px`
  indicator.style.transform = `translateX(${active.offsetLeft - indicator.offsetLeft}px)`
  if (!animate) requestAnimationFrame(() => { indicator.style.transition = '' })
}

export function setChecked(root, value) {
  root.querySelectorAll('[role="radio"]').forEach(b => {
    b.setAttribute('aria-checked', String(b.dataset.value === value))
  })
}

export function isEditable(target) {
  return target instanceof HTMLElement &&
    (target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName))
}
