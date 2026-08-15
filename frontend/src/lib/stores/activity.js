import { writable, derived } from 'svelte/store';

// Count of network requests currently in flight. Every call through api.js
// increments on the way out and decrements on the way back, which is what drives
// the thin progress bar under the navbar — the same "something is happening"
// signal Gmail and Fastmail show.
const count = writable(0);

export const inFlight = { subscribe: count.subscribe };

export const busy = derived(count, ($c) => $c > 0);

export function begin() {
  count.update((n) => n + 1);
}

export function end() {
  count.update((n) => Math.max(0, n - 1));
}

/** Run `fn` while counting it as activity. Always balances, even on throw. */
export async function tracked(fn) {
  begin();
  try {
    return await fn();
  } finally {
    end();
  }
}
