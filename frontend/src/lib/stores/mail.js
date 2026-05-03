import { writable } from 'svelte/store';
import { browser } from '$app/environment';

function createDarkModeStore() {
  const prefersDark = browser ? window.matchMedia('(prefers-color-scheme: dark)').matches : false;
  const stored = browser ? localStorage.getItem('darkMode') : null;
  const initial = stored !== null ? stored === 'true' : prefersDark;

  const { subscribe, set, update } = writable(initial);
  return {
    subscribe,
    toggle() {
      update((v) => {
        const next = !v;
        if (browser) {
          localStorage.setItem('darkMode', String(next));
          document.documentElement.classList.toggle('dark', next);
        }
        return next;
      });
    },
    init(value) {
      if (browser) document.documentElement.classList.toggle('dark', value);
      set(value);
    }
  };
}

function persisted(key, defaultValue, { min, max } = {}) {
  const stored = browser ? localStorage.getItem(key) : null;
  let initial = stored !== null ? JSON.parse(stored) : defaultValue;
  if (min != null) initial = Math.max(min, initial);
  if (max != null) initial = Math.min(max, initial);

  const store = writable(initial);
  if (browser) {
    store.subscribe((v) => localStorage.setItem(key, JSON.stringify(v)));
  }
  return store;
}

export const appName          = writable('JMAP Mail');
export const currentUser      = writable('');
export const darkMode         = createDarkModeStore();
export const selectedMailbox  = writable(null);
export const selectedEmailId  = writable(null);
export const emails           = writable([]);
export const mailboxes        = writable([]);
export const loading          = writable(false);
export const composeOpen      = writable(false);
export const composeContext   = writable(null);  // { mode, to, subject } | null
export const jmapSession      = writable(null);
export const jmapAccountId    = writable(null);

// Move-to-folder / drag state
export const draggedEmailId   = writable(null);
export const contextMenu      = writable(null); // { x, y, emailId } | null
export const movePickerOpen   = writable(null); // emailId to move, or null

// Modal visibility
export const sieveOpen           = writable(false);
export const newFolderOpen       = writable(false);
export const appPasswordsOpen    = writable(false);

// Search
export const searchQuery      = writable('');

// Panel widths — persisted to localStorage, clamped to sane ranges
export const sidebarWidth     = persisted('sidebarWidth',     240, { min: 180, max: 380 });
export const messageListWidth = persisted('messageListWidth', 320, { min: 220, max: 520 });
