const bus = new EventTarget()
// event → (handler → wrapped listener), so `off` can find what `on` added.
const listeners = new Map()

export const store = {
  emit(event, data) {
    bus.dispatchEvent(new CustomEvent(event, { detail: data }))
  },

  /** Subscribes; returns an unsubscribe function. */
  on(event, handler) {
    const wrapped = e => handler(e.detail)
    if (!listeners.has(event)) listeners.set(event, new Map())
    listeners.get(event).set(handler, wrapped)
    bus.addEventListener(event, wrapped)
    return () => store.off(event, handler)
  },

  off(event, handler) {
    const wrapped = listeners.get(event)?.get(handler)
    if (!wrapped) return
    bus.removeEventListener(event, wrapped)
    listeners.get(event).delete(handler)
  },
}
