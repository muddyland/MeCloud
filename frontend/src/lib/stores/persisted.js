import { writable } from 'svelte/store';
import { browser } from '$app/environment';

/**
 * A store mirrored to localStorage, for per-device display preferences.
 *
 * Every read and write is guarded. These run at module load, so a corrupt or
 * hand-edited entry that threw here used to blank the whole app before it had
 * drawn anything; storage can also be disabled outright (private windows,
 * blocked site data). In every such case the default is used and the app
 * carries on — a lost pane width is not worth an error.
 *
 * @param {string} key
 * @param {*} defaultValue
 * @param {object} [opts]
 * @param {Array}  [opts.allowed]  the only values accepted from storage
 * @param {number} [opts.min]      numeric clamp
 * @param {number} [opts.max]
 */
export function readPersisted(raw, defaultValue, { allowed = null, min, max } = {}) {
  if (raw === null || raw === undefined) return defaultValue;
  let value;
  try {
    value = JSON.parse(raw);
  } catch {
    return defaultValue;
  }
  if (allowed) return allowed.includes(value) ? value : defaultValue;
  if (typeof value !== typeof defaultValue) return defaultValue;
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) return defaultValue;
    if (min != null) value = Math.max(min, value);
    if (max != null) value = Math.min(max, value);
  }
  return value;
}

export function persisted(key, defaultValue, opts = {}) {
  let raw = null;
  if (browser) {
    try { raw = localStorage.getItem(key); } catch { /* storage blocked */ }
  }
  const store = writable(readPersisted(raw, defaultValue, opts));
  if (browser) {
    store.subscribe((v) => {
      try { localStorage.setItem(key, JSON.stringify(v)); } catch { /* full or blocked */ }
    });
  }
  return store;
}
