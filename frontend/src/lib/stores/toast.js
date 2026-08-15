import { writable } from 'svelte/store';

const { subscribe, update } = writable([]);
let nextId = 0;

// Dismissing a toast by hand used to leave its auto-dismiss timer running. It
// was harmless in practice but it kept a closure alive per toast, and it meant
// a fast-clicking user accumulated timers that fired against nothing.
const timers = new Map();

export const toasts = { subscribe };

export function toast(message, type = 'info', duration = 3500) {
  const id = ++nextId;
  update(list => [...list, { id, type, message }]);
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

export function clearToasts() {
  for (const timer of timers.values()) clearTimeout(timer);
  timers.clear();
  update(() => []);
}
