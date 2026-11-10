import { writable, derived } from 'svelte/store';
import { persisted } from './persisted.js';
import { compareNodes } from '#lib/fileTypes.js';
import { emptyIndex, searchNodes, tagCounts as countTags, withoutReserved } from '#lib/tags.js';

/** Every FileNode in the account, flat. The tree is derived from parentId. */
export const fileNodes     = writable([]);
export const filesLoading  = writable(false);
export const filesError    = writable('');

/** null = the root of the drive. */
export const currentFolderId = writable(null);
export const selectedFileIds = writable(new Set());
export const previewNode     = writable(null);
export const fileSearch      = writable('');

/** Right-click menu in the listing: { x, y, nodeId, ids } | null. */
export const fileContextMenu = writable(null);

/** Folders the sidebar tree has expanded. */
export const expandedFolders = writable(new Set());

export const viewMode = persisted('filesViewMode', 'grid', { allowed: ['grid', 'list'] });
export const sortKey  = persisted('filesSortKey', 'name', { allowed: ['name', 'size', 'modified'] });
export const sortAsc  = persisted('filesSortAsc', true, { allowed: [true, false] });

/** In-flight uploads: { id, name, progress, error, done }. */
export const uploads = writable([]);

/**
 * The tag index (see lib/tags.js) and the FileNode it was read from, so a
 * reload can tell whether it changed without downloading it again.
 * `loaded` is false until the first read finishes; `error` is shown to the user.
 */
export const tagIndex = writable({ index: emptyIndex(), node: null, loaded: false, error: '' });

// ── Derived views ───────────────────────────────────────────────────────────

/**
 * The nodes the user sees: everything but MeCloud's own `.mecloud` folder.
 * `fileNodes` keeps it, because it is real storage and the index has to be
 * findable; every listing, tree and search reads this instead.
 */
export const visibleFileNodes = derived(fileNodes, ($nodes) => withoutReserved($nodes));

export const nodesById = derived(fileNodes, ($nodes) => {
  const map = new Map();
  for (const node of $nodes) map.set(node.id, node);
  return map;
});

function groupByParent(nodes) {
  const map = new Map();
  for (const node of nodes) {
    const key = node.parentId ?? null;
    if (!map.has(key)) map.set(key, []);
    map.get(key).push(node);
  }
  return map;
}

/** Children per folder, as displayed — the reserved folder is not among them. */
export const childrenByParent = derived(visibleFileNodes, groupByParent);

/**
 * Children per folder including the hidden ones. For name-clash checks: a
 * hidden `.mecloud` still holds its name, and an upload must not collide with it.
 */
export const allChildrenByParent = derived(fileNodes, groupByParent);

/** id → tags, for every node with any. */
export const fileTags = derived(tagIndex, ($t) => $t.index.tags);

/** Every tag with how many visible items carry it, alphabetical. */
export const tagCounts = derived([fileTags, visibleFileNodes], ([$tags, $nodes]) =>
  countTags($tags, new Set($nodes.map((n) => n.id))));

/** The listing for the folder on screen, searched and sorted. */
export const visibleNodes = derived(
  [childrenByParent, visibleFileNodes, currentFolderId, fileSearch, sortKey, sortAsc, fileTags],
  ([$children, $all, $folder, $search, $key, $asc, $tags]) => {
    const query = $search.trim();
    // Search spans the whole drive, the way Finder and Drive do — scoping it to
    // the open folder makes search useless for actually finding things. It
    // matches tags as well as names, and `#tag` filters by tag exactly.
    const pool = query
      ? searchNodes($all, $tags, query)
      : ($children.get($folder ?? null) ?? []);
    return [...pool].sort((a, b) => compareNodes(a, b, { key: $key, ascending: $asc }));
  },
);

export const totalUsage = derived(fileNodes, ($nodes) =>
  $nodes.reduce((sum, n) => sum + (Number(n.size) || 0), 0));
