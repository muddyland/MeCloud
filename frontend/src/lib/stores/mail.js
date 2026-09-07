import { writable, derived } from 'svelte/store';
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

export const appName          = writable('MeCloud');
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

// Multi-select state
export const selectedEmailIds = writable(new Set()); // Set<emailId>

// Move-to-folder / drag state
export const draggedEmailId   = writable(null);
export const contextMenu      = writable(null); // { x, y, emailId } | null
export const movePickerOpen   = writable(null); // emailId | emailId[] to move, or null

// Modal visibility
export const sieveOpen           = writable(false);
export const newFolderOpen       = writable(false);
export const appPasswordsOpen    = writable(false);
export const shortcutsOpen       = writable(false);
export const helpOpen            = writable(false);
export const commandPaletteOpen  = writable(false);

// True while any modal owns the screen — keyboard shortcuts stand down so they
// don't fire while the user is typing into a dialog.
export const anyModalOpen = derived(
  [composeOpen, sieveOpen, newFolderOpen, appPasswordsOpen, shortcutsOpen,
   helpOpen, commandPaletteOpen, movePickerOpen, contextMenu],
  ([$compose, $sieve, $folder, $passwords, $shortcuts, $help, $palette, $picker, $menu]) =>
    $compose || $sieve || $folder || $passwords || $shortcuts || $help || $palette
    || $picker !== null || $menu !== null
);

// The list the user is actually looking at (search results or the mailbox),
// published by MessageList so keyboard navigation can walk it.
export const visibleEmails = writable([]);

// Actions for the message currently in the reading pane, registered by
// MessagePane. Lets r/a/f/# reach the open message without prop-drilling.
export const messageActions = writable(null);

// Registered by the mail page so the list's refresh button (and the `.`
// shortcut) can trigger the same background refresh the event stream does.
export const mailRefresher = writable(null);

// Search
export const searchQuery      = writable('');

// Unread count for the browser tab title — inbox only, matching what other
// mail clients badge.
export const inboxUnread = derived(mailboxes, ($mailboxes) => {
  const inbox = $mailboxes.find((m) => m.role === 'inbox');
  return inbox?.unreadEmails ?? 0;
});

// Panel widths — persisted to localStorage, clamped to sane ranges
export const sidebarWidth     = persisted('sidebarWidth',     240, { min: 180, max: 380 });
export const messageListWidth = persisted('messageListWidth', 320, { min: 220, max: 520 });
