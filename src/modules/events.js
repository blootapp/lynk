export const EVENTS = {
  // Download state (mirrors backend events)
  DOWNLOADS_LOADED:  'downloads:loaded',   // Download[]
  DOWNLOAD_UPDATED:  'download:updated',   // Download
  DOWNLOAD_REMOVED:  'download:removed',   // id
  DOWNLOAD_ACTION:   'download:action',    // { action, id?, groupId? }
  DOWNLOAD_FOCUS:    'download:focus',     // id — scroll to + highlight

  // Composer
  SUBMIT_URLS:       'composer:submit',    // { text, format }
  SUBMIT_DONE:       'composer:done',      // { ok, reason? }

  // App
  PREF_UPDATED:      'pref:updated',       // { key, value }
  ENGINE_STATUS:     'engine:status',      // EngineStatus
  PANEL_OPEN:        'panel:open',         // 'history' | 'settings'
  PANEL_OPENED:      'panel:opened',       // name
  PANEL_CLOSED:      'panel:closed',       // name
  TOAST:             'toast:show',         // { message, kind?, icon? }
}
