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

/** null = "wherever the top of the drive is" — see rootNodeId. */
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

/**
 * The id of the tree's root node, or null if this server has no single root.
 *
 * draft-ietf-jmap-filenode section 3.1 defines parentId as "the Id of the
 * parent node, or null if this is the root node" — null identifies *the root
 * itself*, not "sits at the top level". This app had read it the second way and
 * created top-level folders with `parentId: null`, which under the spec's
 * reading declares a second root rather than a child of the existing one. Such
 * a node is real enough to occupy its name against its siblings while never
 * appearing under the root the UI actually walks.
 *
 * Servers differ here, so this detects rather than assumes: a single parentless
 * collection is a real root and gets used as the default parent. Anything else
 * — several parentless nodes, or a parentless file — means the server is using
 * the looser "null is the top level" reading, and this stays null so every
 * parent position behaves exactly as it did before.
 */
export const rootNodeId = derived(fileNodes, ($nodes) => {
  // Strictly: a root is the *only* parentless node, and it is a collection.
  // Picking the likeliest candidate out of several was tried and is a trap —
  // "the parentless folder that has children" also describes a perfectly
  // ordinary top-level folder on a server using the looser reading, so it
  // mislabels a healthy drive. Ambiguity here returns null, which leaves every
  // parentless node listed at the top exactly as before: orphans included, and
  // therefore visible and deletable.
  const parentless = $nodes.filter((n) => (n.parentId ?? null) === null);
  if (parentless.length !== 1) return null;
  const [root] = parentless;
  return (root.blobId ?? null) === null ? root.id : null;
});

/**
 * Parentless nodes that are not the root.
 *
 * Under the spec's reading these should not exist. Where they do, they are
 * unreachable from the root, so a tree walk never shows them and they cannot
 * be deleted — while still occupying their names. They are surfaced in the
 * root listing so they can be dealt with.
 */
export const orphanNodes = derived([fileNodes, rootNodeId], ([$nodes, $root]) => {
  if (!$root) return [];
  return $nodes.filter((n) => (n.parentId ?? null) === null && n.id !== $root);
});

/** The parent that "here" refers to: the open folder, else the root. */
export const activeParentId = derived(
  [currentFolderId, rootNodeId],
  ([$folder, $root]) => $folder ?? $root ?? null,
);

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
  [childrenByParent, fileNodes, activeParentId, fileSearch, sortKey, sortAsc, rootNodeId, orphanNodes],
  ([$children, $all, $folder, $search, $key, $asc, $root, $orphans]) => {
    const query = $search.trim().toLowerCase();
    // Search spans the whole drive, the way Finder and Drive do — scoping it to
    // the open folder makes search useless for actually finding things.
    const pool = query
      ? $all.filter((n) => String(n.name ?? '').toLowerCase().includes(query))
      : [
          ...($children.get($folder ?? null) ?? []),
          // Orphans belong nowhere, so show them at the root: visible is the
          // only state from which they can be removed.
          ...($root && $folder === $root ? $orphans : []),
        ];
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
