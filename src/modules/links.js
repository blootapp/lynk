// Turning pasted / dropped text into downloadable URLs.

const BARE_DOMAIN = /^(?:[\w-]+\.)+[a-z]{2,}(?::\d+)?(?:[/?#]\S*)?$/i
const TRACKING = /^(utm_\w+|si|feature|pp|igsh|igshid|fbclid|gclid|ref|ref_src|ref_url|s|t)$/i

/**
 * Every http(s) link in `text`, deduplicated, in order. Accepts bare domains
 * ("youtu.be/abc") and links wrapped in <>, quotes or brackets.
 */
export function extractUrls(text) {
  if (!text) return []
  const out = []
  const seen = new Set()
  for (const raw of String(text).split(/\s+/)) {
    let token = raw.trim()
      .replace(/^[<("'[]+/, '')
      .replace(/[>)"'\],.;!]+$/, '')
    if (!token || token.startsWith('#')) continue
    if (!/^https?:\/\//i.test(token)) {
      if (!BARE_DOMAIN.test(token)) continue
      token = `https://${token}`
    }
    let url
    try { url = new URL(token) } catch { continue }
    if (url.protocol !== 'http:' && url.protocol !== 'https:') continue
    if (!url.hostname.includes('.') && url.hostname !== 'localhost') continue
    const href = url.href
    if (!seen.has(href)) { seen.add(href); out.push(href) }
  }
  return out
}

/** Comparable form of a URL for duplicate detection. */
export function normalizeUrl(href) {
  try {
    const u = new URL(href)
    let host = u.hostname.toLowerCase().replace(/^(www|m|music)\./, '')
    let path = u.pathname.replace(/\/+$/, '')
    const params = new URLSearchParams(u.search)
    // youtu.be/ID and /shorts/ID → watch?v=ID
    if (host === 'youtu.be') { params.set('v', path.slice(1)); host = 'youtube.com'; path = '/watch' }
    const shorts = host === 'youtube.com' && path.match(/^\/shorts\/([\w-]+)/)
    if (shorts) { params.set('v', shorts[1]); path = '/watch' }
    for (const k of [...params.keys()]) if (TRACKING.test(k)) params.delete(k)
    params.sort()
    const q = params.toString()
    return `${host}${path}${q ? `?${q}` : ''}`
  } catch {
    return href
  }
}
