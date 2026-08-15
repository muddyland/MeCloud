import { browser } from '$app/environment';

/**
 * Senders the user has chosen to always load remote images for.
 *
 * Kept in localStorage rather than on the server: it is a per-device display
 * preference, it contains no secrets, and putting it in the JMAP account would
 * mean inventing a storage schema for it.
 */
const KEY = 'trustedImageSenders';
const MAX = 500;   // bounded so the list cannot grow without limit

function read() {
  if (!browser) return [];
  try {
    const parsed = JSON.parse(localStorage.getItem(KEY) ?? '[]');
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function normalise(address) {
  return String(address ?? '').trim().toLowerCase();
}

export function isTrustedSender(address) {
  const key = normalise(address);
  return !!key && read().includes(key);
}

export function trustSender(address) {
  const key = normalise(address);
  if (!browser || !key) return;
  const list = read().filter((a) => a !== key);
  list.push(key);
  try {
    localStorage.setItem(KEY, JSON.stringify(list.slice(-MAX)));
  } catch {
    // Storage full or blocked — the preference just won't persist.
  }
}

export function untrustSender(address) {
  const key = normalise(address);
  if (!browser || !key) return;
  try {
    localStorage.setItem(KEY, JSON.stringify(read().filter((a) => a !== key)));
  } catch {
    // Ignore — nothing we can do, and nothing breaks.
  }
}
