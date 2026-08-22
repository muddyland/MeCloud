import { readable, writable, derived } from 'svelte/store';
import { browser } from '$app/environment';

/**
 * A store that tracks a CSS media query.
 *
 * SSR-safe: it starts at `initial` and only subscribes to matchMedia in the
 * browser, so prerendering never touches `window`.
 */
export function mediaQuery(query, initial = false) {
  return readable(initial, (set) => {
    if (!browser) return;
    const mql = window.matchMedia(query);
    set(mql.matches);
    const onChange = (event) => set(event.matches);
    mql.addEventListener('change', onChange);
    return () => mql.removeEventListener('change', onChange);
  });
}

/**
 * True below Tailwind's `lg` breakpoint (1024px).
 *
 * That is where the three-pane mail layout stops fitting: a 240px sidebar plus
 * a 320px message list leaves nothing for the reading pane, and on a phone the
 * sidebar alone eats most of the viewport. Below this the app switches to a
 * single visible pane with drill-down navigation.
 */
export const isCompact = mediaQuery('(max-width: 1023.98px)');

/** True for touch-primary devices, used to enlarge hit targets. */
export const isCoarsePointer = mediaQuery('(pointer: coarse)');

/** Whether the off-canvas sidebar is showing. Only meaningful when compact. */
export const sidebarOpen = writable(false);

/**
 * How many drawers are mounted. Pages register their own, so the navbar can
 * offer the toggle only where it does something — Notes and the dashboard have
 * no drawer, and a control that visibly does nothing is worse than no control.
 */
const sidebarCount = writable(0);

export const hasSidebar = derived(sidebarCount, ($n) => $n > 0);

/** Call on mount; the returned function unregisters on destroy. */
export function registerSidebar() {
  sidebarCount.update((n) => n + 1);
  return () => sidebarCount.update((n) => Math.max(0, n - 1));
}

export function openSidebar()  { sidebarOpen.set(true); }
export function closeSidebar() { sidebarOpen.set(false); }
export function toggleSidebar() { sidebarOpen.update((v) => !v); }

/**
 * The sidebar is an overlay only while compact — at desktop widths it is part
 * of the layout and is always present regardless of the drawer flag.
 */
// Leaving the flag set while widening means shrinking again silently reopens
// the drawer over the content.
if (browser) {
  isCompact.subscribe((compact) => {
    if (!compact) sidebarOpen.set(false);
  });
}

export const sidebarIsOverlay = derived(
  [isCompact, sidebarOpen],
  ([$compact, $open]) => $compact && $open,
);
