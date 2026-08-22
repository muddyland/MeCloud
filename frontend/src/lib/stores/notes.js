import { writable, derived } from 'svelte/store';
import { noteTitle, excerpt } from '$lib/markdown.js';

/** Markdown FileNodes under the Notes folder. */
export const noteNodes     = writable([]);
export const notesFolder   = writable(null);
export const notesLoading  = writable(false);
export const notesError    = writable('');

export const selectedNoteId = writable(null);
export const noteSearch     = writable('');

/** Source text of the open note, keyed by node id so switching is instant. */
export const noteCache = writable(new Map());

/** 'preview' | 'edit' | 'split' — persisted per device. */
export const noteView = writable('preview');

/** 'idle' | 'dirty' | 'saving' | 'saved' | 'error' */
export const saveState = writable('idle');
export const saveError = writable('');

export const selectedNote = derived(
  [noteNodes, selectedNoteId],
  ([$nodes, $id]) => $nodes.find((n) => n.id === $id) ?? null,
);

/**
 * The note list, searched and sorted.
 *
 * Search covers the title and the cached body — a note whose text has not been
 * opened yet can only match on its filename, which is worth knowing rather than
 * pretending otherwise.
 */
export const visibleNotes = derived(
  [noteNodes, noteSearch, noteCache],
  ([$nodes, $search, $cache]) => {
    const query = $search.trim().toLowerCase();
    const decorated = $nodes.map((node) => {
      const text = $cache.get(node.id) ?? '';
      return {
        node,
        title: noteTitle(node.name, text),
        preview: text ? excerpt(text, 100) : '',
        folderPath: node.folderPath ?? '',
      };
    });

    const matched = query
      ? decorated.filter(({ node, title, preview }) =>
          title.toLowerCase().includes(query)
          || String(node.name ?? '').toLowerCase().includes(query)
          || preview.toLowerCase().includes(query))
      : decorated;

    // Most recently modified first — the useful default for notes, unlike files.
    return matched.sort(
      (a, b) => new Date(b.node.modified ?? 0) - new Date(a.node.modified ?? 0),
    );
  },
);

export function cacheNote(id, text) {
  noteCache.update((map) => {
    const next = new Map(map);
    next.set(id, text);
    return next;
  });
}

export function forgetNote(id) {
  noteCache.update((map) => {
    const next = new Map(map);
    next.delete(id);
    return next;
  });
}

export function resetNotesState() {
  noteNodes.set([]);
  notesFolder.set(null);
  selectedNoteId.set(null);
  noteSearch.set('');
  noteCache.set(new Map());
  saveState.set('idle');
  saveError.set('');
  notesError.set('');
}
