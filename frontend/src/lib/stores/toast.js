import { writable } from 'svelte/store';

const { subscribe, update } = writable([]);
let nextId = 0;

// Dismissing a toast by hand used to leave its auto-dismiss timer running. It
// was harmless in practice but it kept a closure alive per toast, and it meant
// a fast-clicking user accumulated timers that fired against nothing.
const timers = new Map();

export const toasts = { subscribe };

/**
 * Show a toast. `action` adds one button — `{ label, run }` — which runs once
 * and dismisses the toast.
 */
export function toast(message, type = 'info', duration = 3500, action = null) {
  const id = ++nextId;
  update(list => [...list, { id, type, message, action }]);
  if (duration > 0) {
    timers.set(id, setTimeout(() => dismiss(id), duration));
  }
  return id;
}

export function dismiss(id) {
  const timer = timers.get(id);
  if (timer !== undefined) {
    clearTimeout(timer);
    timers.delete(id);
  }
  update(list => list.filter(t => t.id !== id));
}

/**
 * A success toast with an Undo button, held a little longer than usual so
 * there is time to reach for it. `undo` may be async; a failure surfaces as an
 * error toast rather than vanishing.
 */
export function undoToast(message, undo, duration = 7000) {
  return toast(message, 'success', duration, {
    label: 'Undo',
    run: async () => {
      try {
        await undo();
      } catch (e) {
        toast(e?.message ?? 'Could not undo that.', 'error');
      }
    },
  });
}

/** Run a toast's action, once, and dismiss it. */
export function runToastAction(t) {
  dismiss(t.id);
  t.action?.run?.();
}

export function clearToasts() {
  for (const timer of timers.values()) clearTimeout(timer);
  timers.clear();
  update(() => []);
}
