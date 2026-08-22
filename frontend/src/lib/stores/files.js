import { writable, derived } from 'svelte/store';
import { browser } from '$app/environment';
import { compareNodes } from '$lib/fileTypes.js';

function persisted(key, defaultValue, allowed = null) {
  const stored = browser ? localStorage.getItem(key) : null;
  let initial = defaultValue;
  if (stored !== null) {
    try {
      const parsed = JSON.parse(stored);
      if (!allowed || allowed.includes(parsed)) initial = parsed;
    } catch {
      // Corrupt entry — fall back to the default rather than throwing at boot.
    }
  }
  const store = writable(initial);
  if (browser) store.subscribe((v) => localStorage.setItem(key, JSON.stringify(v)));
  return store;
}

/** Every FileNode in the account, flat. The tree is derived from parentId. */
export const fileNodes     = writable([]);
export const filesLoading  = writable(false);
export const filesError    = writable('');

/** null = the root of the drive. */
export const currentFolderId = writable(null);
export const selectedFileIds = writable(new Set());
export const previewNode     = writable(null);
export const fileSearch      = writable('');

/** Folders the sidebar tree has expanded. */
export const expandedFolders = writable(new Set());

export const viewMode = persisted('filesViewMode', 'grid', ['grid', 'list']);
export const sortKey  = persisted('filesSortKey', 'name', ['name', 'size', 'modified']);
export const sortAsc  = persisted('filesSortAsc', true, [true, false]);

/** In-flight uploads: { id, name, progress, error, done }. */
export const uploads = writable([]);

// ── Derived views ───────────────────────────────────────────────────────────

export const nodesById = derived(fileNodes, ($nodes) => {
  const map = new Map();
  for (const node of $nodes) map.set(node.id, node);
  return map;
});

export const childrenByParent = derived(fileNodes, ($nodes) => {
  const map = new Map();
  for (const node of $nodes) {
    const key = node.parentId ?? null;
    if (!map.has(key)) map.set(key, []);
    map.get(key).push(node);
  }
  return map;
});

/** The listing for the folder on screen, searched and sorted. */
export const visibleNodes = derived(
  [childrenByParent, fileNodes, currentFolderId, fileSearch, sortKey, sortAsc],
  ([$children, $all, $folder, $search, $key, $asc]) => {
    const query = $search.trim().toLowerCase();
    // Search spans the whole drive, the way Finder and Drive do — scoping it to
    // the open folder makes search useless for actually finding things.
    const pool = query
      ? $all.filter((n) => String(n.name ?? '').toLowerCase().includes(query))
      : ($children.get($folder ?? null) ?? []);
    return [...pool].sort((a, b) => compareNodes(a, b, { key: $key, ascending: $asc }));
  },
);

export const folderCount = derived(fileNodes, ($nodes) =>
  $nodes.filter((n) => n.blobId === null || n.blobId === undefined).length);

export const totalUsage = derived(fileNodes, ($nodes) =>
  $nodes.reduce((sum, n) => sum + (Number(n.size) || 0), 0));

export function resetFilesState() {
  fileNodes.set([]);
  currentFolderId.set(null);
  selectedFileIds.set(new Set());
  previewNode.set(null);
  fileSearch.set('');
  filesError.set('');
}
