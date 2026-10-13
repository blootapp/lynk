import { t } from './i18n.js'

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB']

/** Sizes in binary units, labelled the way Explorer labels them. */
export function formatBytes(n) {
  if (!Number.isFinite(n) || n <= 0) return ''
  let i = 0
  while (n >= 1024 && i < UNITS.length - 1) { n /= 1024; i++ }
  const digits = i >= 2 && n < 100 ? 1 : 0
  return `${n.toFixed(digits)} ${UNITS[i]}`
}

export function formatSpeed(bps) {
  const s = formatBytes(bps)
  return s ? `${s}/s` : ''
}

export function formatEta(sec) {
  if (!Number.isFinite(sec) || sec < 0) return ''
  sec = Math.round(sec)
  if (sec < 60) return t.left(`${Math.max(sec, 1)}s`)
  if (sec < 3600) return t.left(`${Math.round(sec / 60)}m`)
  return t.left(`${Math.floor(sec / 3600)}h ${Math.round((sec % 3600) / 60)}m`)
}

export function timeOfDay(ms) {
  return new Date(ms).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
}

export function dayLabel(ms) {
  const d = new Date(ms)
  const today = new Date()
  const startOf = x => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime()
  const diff = Math.round((startOf(today) - startOf(d)) / 86400000)
  if (diff <= 0) return t.today
  if (diff === 1) return t.yesterday
  if (diff < 7) return d.toLocaleDateString(undefined, { weekday: 'long' })
  const sameYear = d.getFullYear() === today.getFullYear()
  return d.toLocaleDateString(undefined, sameYear
    ? { month: 'short', day: 'numeric' }
    : { month: 'short', day: 'numeric', year: 'numeric' })
}

export function relativeTime(ms) {
  const sec = Math.round((Date.now() - ms) / 1000)
  if (sec < 60) return 'just now'
  const plural = (n, unit) => `${n} ${unit}${n === 1 ? '' : 's'} ago`
  const min = Math.round(sec / 60)
  if (min < 60) return plural(min, 'minute')
  const h = Math.round(min / 60)
  if (h < 24) return plural(h, 'hour')
  return plural(Math.round(h / 24), 'day')
}

const SITES = {
  youtube: 'YouTube', youtubetab: 'YouTube', youtubeclip: 'YouTube', tiktok: 'TikTok',
  instagram: 'Instagram', twitter: 'X', facebook: 'Facebook', vimeo: 'Vimeo',
  soundcloud: 'SoundCloud', bandcamp: 'Bandcamp', dailymotion: 'Dailymotion',
  twitch: 'Twitch', reddit: 'Reddit', pinterest: 'Pinterest', bilibili: 'Bilibili',
  vk: 'VK', rumble: 'Rumble', kick: 'Kick', threads: 'Threads', bluesky: 'Bluesky',
  tumblr: 'Tumblr', streamable: 'Streamable', mixcloud: 'Mixcloud', audiomack: 'Audiomack',
  linkedin: 'LinkedIn', snapchat: 'Snapchat', imgur: 'Imgur', ted: 'TED', bbc: 'BBC',
  odnoklassniki: 'OK', rutube: 'Rutube', niconico: 'Niconico', applepodcasts: 'Apple Podcasts',
}

/** Human site name from yt-dlp's extractor key, falling back to the host. */
export function siteName(rec) {
  const key = (rec.site || '').toLowerCase()
  if (key && key !== 'generic') {
    if (SITES[key]) return SITES[key]
    const base = key.replace(/(vod|clips?|tab|video|post|playlist|album|track|user|channel|live|stream|shorts|ie|search)+$/, '')
    if (SITES[base]) return SITES[base]
  }
  return hostOf(rec.url)
}

export function hostOf(url) {
  try { return new URL(url).hostname.replace(/^www\.|^m\./, '') } catch { return '' }
}

export function qualityShort(q) {
  if (!q || q === 'best') return 'Best'
  if (q === '2160p') return '4K'
  return q
}

/** "MP4 1080p" / "MKV" / "MP3" — what the user asked for (or got). */
export function formatLabel(rec) {
  if (rec.format === 'audio') return (rec.ext || rec.audio_format || 'mp3').toUpperCase()
  const ext = (rec.ext || rec.video_format || 'mp4').toUpperCase()
  return rec.quality && rec.quality !== 'best' ? `${ext} ${qualityShort(rec.quality)}` : ext
}
