import { writable } from 'svelte/store';

const { subscribe, update } = writable([]);
let nextId = 0;

export const toasts = { subscribe };

export function toast(message, type = 'info', duration = 3500) {
  const id = ++nextId;
  update(list => [...list, { id, type, message }]);
  setTimeout(() => dismiss(id), duration);
}

export function dismiss(id) {
  update(list => list.filter(t => t.id !== id));
}
