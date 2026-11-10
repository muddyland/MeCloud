// The current page as a store, for the components still written in Svelte's
// legacy syntax.
//
// SvelteKit 3 removed $app/stores in favour of $app/state, whose `page` is
// runes state. A legacy `$:` statement only re-runs for stores and the
// component's own variables, so reading `page.url` there directly would stop
// updating on navigation. This keeps `$page.url` working in those components
// until they move to runes.
import { page as state } from '$app/state';
import { toStore } from 'svelte/store';

export const page = toStore(() => ({ url: state.url }));
