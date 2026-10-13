import { esc } from './dom.js'

/**
 * Bloot-style confirm dialog (§6.3). Resolves true on confirm, false on
 * cancel, Escape or a click outside.
 */
export function confirm({ title, message, confirmLabel = 'Confirm', cancelLabel = 'Cancel', destructive = false }) {
  return new Promise(resolve => {
    const root = document.getElementById('overlay-root')
    const previousFocus = document.activeElement
    const backdrop = document.createElement('div')
    backdrop.className = 'modal-backdrop'
    backdrop.innerHTML = `
      <div class="modal" role="alertdialog" aria-modal="true" aria-labelledby="modal-title" aria-describedby="modal-body">
        <div class="modal-title" id="modal-title">${esc(title)}</div>
        ${message ? `<div class="modal-body" id="modal-body">${esc(message)}</div>` : ''}
        <div class="modal-actions">
          <button class="btn btn-quiet" data-r="cancel">${esc(cancelLabel)}</button>
          <button class="btn ${destructive ? 'btn-danger' : 'btn-primary'}" data-r="ok">${esc(confirmLabel)}</button>
        </div>
      </div>`

    const close = result => {
      document.removeEventListener('keydown', onKey, true)
      backdrop.remove()
      if (previousFocus instanceof HTMLElement) previousFocus.focus()
      resolve(result)
    }
    const onKey = e => {
      if (e.key === 'Escape') { e.stopPropagation(); e.preventDefault(); close(false) }
      if (e.key === 'Tab') {
        // Keep focus inside the dialog.
        const buttons = [...backdrop.querySelectorAll('button')]
        const i = buttons.indexOf(document.activeElement)
        e.preventDefault()
        buttons[(i + (e.shiftKey ? buttons.length - 1 : 1)) % buttons.length].focus()
      }
    }

    backdrop.addEventListener('mousedown', e => { if (e.target === backdrop) close(false) })
    backdrop.querySelector('[data-r="cancel"]').addEventListener('click', () => close(false))
    backdrop.querySelector('[data-r="ok"]').addEventListener('click', () => close(true))
    document.addEventListener('keydown', onKey, true)
    root.appendChild(backdrop)
    backdrop.querySelector(destructive ? '[data-r="cancel"]' : '[data-r="ok"]').focus()
  })
}
