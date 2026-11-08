/**
 * File tags, stored in the drive itself.
 *
 * JMAP FileNode has no keywords or tags property, and the MeCloud backend is
 * stateless and has to stay that way. So tags live where Notes live: in an
 * ordinary file. One JSON index, `.mecloud/tags.json` at the top of the drive,
 * maps FileNode ids to their tags. Keying by id rather than by path means a
 * rename or a move keeps the tags without anyone having to rewrite the index.
 *
 * The desktop client implements the same format in Rust, so the rules here are
 * written to be mirrored exactly: the pure functions come first and carry the
 * whole behaviour, and the JMAP functions at the bottom only fetch and store.
 *
 *   { "version": 1, "tags": { "<FileNode id>": ["Tax", "Insurance 2026"] } }
 */
import { jmapPost } from './api.js';
import {
  fetchFileNodes, fetchBlobBytes, uploadBlob, createFolder, createFile, updateNode,
  normalizeNode, maxObjectsInGet, fileUsing,
} from './files.js';
import { fileKind } from './fileTypes.js';

/** The reserved folder at the top of the drive, and the index inside it. */
export const RESERVED_FOLDER = '.mecloud';
export const TAG_INDEX_NAME = 'tags.json';
export const TAG_INDEX_TYPE = 'application/json';
export const TAG_INDEX_VERSION = 1;

export const MAX_TAG_LENGTH = 40;
export const MAX_TAGS_PER_FILE = 20;
export const MAX_WRITE_ATTEMPTS = 5;

export const MALFORMED_MESSAGE =
  'The tags file (.mecloud/tags.json) could not be read, so tags are hidden and '
  + 'cannot be changed until it is fixed or removed.';
export const NEWER_VERSION_MESSAGE =
  'These tags were saved by a newer version of MeCloud, so they are read-only here.';
export const GAVE_UP_MESSAGE =
  'Could not save the tags: the drive kept changing while they were being saved. Try again.';

// ── Tag rules ───────────────────────────────────────────────────────────────
//
// Whitespace is spelled out rather than left to `\s`, because JavaScript's
// `\s` and Rust's `char::is_whitespace` disagree on two characters (U+FEFF and
// U+0085), and a tag must normalise identically on both sides. This is
// Unicode's White_Space property, which is what Rust uses.

const WS = '\\t\\n\\v\\f\\r \\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000';
const WS_EDGES = new RegExp(`^[${WS}]+|[${WS}]+$`, 'g');
const WS_RUN = new RegExp(`[${WS}]+`, 'g');
// Unicode general category Cc, the same set as Rust's `char::is_control`.
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/;

const trimWs = (s) => s.replace(WS_EDGES, '');

/**
 * One tag as typed, cleaned up — or null when it cannot be a tag.
 *
 * A leading `#` is stripped because people type tags the way they search for
 * them. A comma is refused rather than kept, because commas are how several
 * tags are typed into one field and a tag containing one could never be typed
 * again. The length counts code points, so an emoji is one character, as it
 * is in Rust.
 */
export function normalizeTag(raw) {
  if (typeof raw !== 'string') return null;
  const tag = trimWs(trimWs(raw).replace(/^#+/, '')).replace(WS_RUN, ' ');
  if (!tag) return null;
  if ([...tag].length > MAX_TAG_LENGTH) return null;
  if (tag.includes(',') || CONTROL.test(tag)) return null;
  return tag;
}

/** Tags keep the case they were typed in but compare without it. */
export const tagKey = (tag) => String(tag).toLowerCase();

/** Split what was typed into a tag field: several tags, comma-separated. */
export function splitTagInput(text) {
  return String(text ?? '').split(/[,\n]/);
}

/**
 * A file's tag list, cleaned: normalised, invalid ones dropped, duplicates
 * (ignoring case) dropped keeping the first, at most MAX_TAGS_PER_FILE.
 *
 * `knownTags` are the spellings already in use. "tax" typed against an index
 * that has "Tax" becomes "Tax", so one tag does not split into two that only
 * differ by a capital letter.
 */
export function canonicalize(tags, knownTags = []) {
  const known = new Map();
  for (const t of knownTags ?? []) {
    const n = normalizeTag(t);
    if (n && !known.has(tagKey(n))) known.set(tagKey(n), n);
  }

  const out = [];
  const seen = new Set();
  for (const raw of tags ?? []) {
    const n = normalizeTag(raw);
    if (!n) continue;
    const key = tagKey(n);
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(known.get(key) ?? n);
    if (out.length >= MAX_TAGS_PER_FILE) break;
  }
  return out;
}

/** Compare by Unicode code point, which is how Rust orders a String. */
export function compareCodePoints(a, b) {
  const x = [...String(a)];
  const y = [...String(b)];
  const n = Math.min(x.length, y.length);
  for (let i = 0; i < n; i += 1) {
    if (x[i] !== y[i]) return x[i].codePointAt(0) - y[i].codePointAt(0);
  }
  return x.length - y.length;
}

/**
 * Every spelling in use, one per tag. Walked in id order so two clients
 * looking at the same index agree on which spelling is "the" spelling.
 */
export function knownTags(tags) {
  const out = new Map();
  for (const id of [...tags.keys()].sort(compareCodePoints)) {
    for (const tag of tags.get(id) ?? []) {
      if (!out.has(tagKey(tag))) out.set(tagKey(tag), tag);
    }
  }
  return [...out.values()];
}

// ── The index file ──────────────────────────────────────────────────────────

/**
 * The in-memory index.
 *
 * `tags` and `extra` are Maps rather than objects: ids and keys come from a
 * file anyone can edit, and a key such as "__proto__" must stay a key.
 * `extra` holds top-level keys this version does not know about, so they can
 * be written back untouched — a newer client may be relying on them.
 */
export function emptyIndex() {
  return { version: TAG_INDEX_VERSION, tags: new Map(), extra: new Map(), writable: true, error: null };
}

function unreadableIndex(message, version = TAG_INDEX_VERSION) {
  return { ...emptyIndex(), version, writable: false, error: message };
}

const isObject = (v) => typeof v === 'object' && v !== null && !Array.isArray(v);

/**
 * Read the index file's text.
 *
 * Never throws: a file that cannot be read shows no tags and refuses writes,
 * because overwriting something we could not understand is how another
 * client's data gets destroyed. `null` (there is no file) is simply empty.
 */
export function parseIndex(text) {
  if (text === null || text === undefined) return emptyIndex();

  let data;
  try {
    data = JSON.parse(String(text));
  } catch {
    return unreadableIndex(MALFORMED_MESSAGE);
  }
  if (!isObject(data)) return unreadableIndex(MALFORMED_MESSAGE);

  const version = data.version === undefined ? TAG_INDEX_VERSION : data.version;
  if (!Number.isInteger(version) || version < 1) return unreadableIndex(MALFORMED_MESSAGE);
  const newer = version > TAG_INDEX_VERSION;

  const raw = data.tags === undefined ? {} : data.tags;
  if (!isObject(raw)) {
    return unreadableIndex(newer ? NEWER_VERSION_MESSAGE : MALFORMED_MESSAGE, version);
  }

  const tags = new Map();
  for (const [id, value] of Object.entries(raw)) {
    // One bad entry should not hide everyone else's tags.
    if (!Array.isArray(value) || !value.every((v) => typeof v === 'string')) continue;
    const list = canonicalize(value);
    if (list.length) tags.set(id, list);
  }

  const extra = new Map(Object.entries(data).filter(([key]) => key !== 'version' && key !== 'tags'));
  return {
    version,
    tags,
    extra,
    writable: !newer,
    error: newer ? NEWER_VERSION_MESSAGE : null,
  };
}

const indent = (json, by) => json.replace(/\n/g, `\n${' '.repeat(by)}`);

/**
 * The index as file text: two-space indent, `tags` sorted by id, a trailing
 * newline — so the file diffs readably when someone looks at it over WebDAV.
 *
 * Assembled by hand rather than with one JSON.stringify, because a JavaScript
 * object puts integer-like keys first whatever order they were added in, and
 * ids are not guaranteed to look unlike integers. Byte for byte this is what
 * serde_json's pretty printer produces for the same map.
 */
export function serializeIndex(index) {
  const ids = [...index.tags.keys()]
    .filter((id) => index.tags.get(id)?.length)
    .sort(compareCodePoints);
  const entries = ids.map(
    (id) => `    ${JSON.stringify(id)}: ${indent(JSON.stringify(index.tags.get(id), null, 2), 4)}`,
  );

  const lines = [
    `  "version": ${TAG_INDEX_VERSION}`,
    `  "tags": ${entries.length ? `{\n${entries.join(',\n')}\n  }` : '{}'}`,
  ];
  for (const [key, value] of index.extra ?? []) {
    if (key === 'version' || key === 'tags' || value === undefined) continue;
    lines.push(`  ${JSON.stringify(key)}: ${indent(JSON.stringify(value, null, 2), 2)}`);
  }
  return `{\n${lines.join(',\n')}\n}\n`;
}

/**
 * Change the tags of one or more nodes. Returns a new index.
 *
 * `set` replaces each node's tags outright (and then `add` and `remove` are
 * ignored); otherwise `remove` goes first, then `add` is appended. Removal is
 * case-insensitive, so removing "tax" removes "Tax". Spellings are settled
 * after the removal, which is what lets a lone "tax" be re-saved as "Tax".
 */
export function applyChange(index, nodeIds, { add = [], remove = [], set = null } = {}) {
  const targets = [...new Set(nodeIds ?? [])].filter((id) => typeof id === 'string' && id);
  const replacing = Array.isArray(set);
  const tags = new Map(index.tags);
  const removeKeys = new Set((remove ?? []).map(normalizeTag).filter(Boolean).map(tagKey));

  const base = new Map();
  for (const id of targets) {
    const current = replacing ? [] : (tags.get(id) ?? []).filter((t) => !removeKeys.has(tagKey(t)));
    base.set(id, current);
    if (current.length) tags.set(id, current); else tags.delete(id);
  }

  const known = knownTags(tags);
  const incoming = replacing ? set : (add ?? []);
  for (const id of targets) {
    const next = canonicalize([...base.get(id), ...incoming], known);
    // A file with no tags has no entry; `[]` is never written.
    if (next.length) tags.set(id, next); else tags.delete(id);
  }
  return { ...index, tags };
}

/**
 * Forget tags for nodes that no longer exist.
 *
 * Only from a complete listing: from a partial one, "not in the listing"
 * means "not seen", and pruning on that would silently delete the tags of
 * everything the server did not return.
 */
export function prune(index, liveIds, { incomplete = false } = {}) {
  if (incomplete || !liveIds) return index;
  const live = liveIds instanceof Set ? liveIds : new Set(liveIds);
  const tags = new Map([...index.tags].filter(([id]) => live.has(id)));
  if (tags.size === index.tags.size) return index;
  return { ...index, tags };
}

/** Tags for one node, never undefined. */
export function tagsFor(tags, id) {
  return tags?.get?.(id) ?? [];
}

/**
 * Every tag with the number of files carrying it, alphabetical. Only ids in
 * `liveIds` count, so a deleted file's leftover entry does not inflate them.
 */
export function tagCounts(tags, liveIds = null) {
  const live = liveIds ? (liveIds instanceof Set ? liveIds : new Set(liveIds)) : null;
  const counts = new Map();
  for (const id of [...(tags?.keys?.() ?? [])].sort(compareCodePoints)) {
    if (live && !live.has(id)) continue;
    for (const tag of tags.get(id)) {
      const key = tagKey(tag);
      const entry = counts.get(key);
      if (entry) entry.count += 1;
      else counts.set(key, { tag, count: 1 });
    }
  }
  return [...counts.values()].sort(
    (a, b) => a.tag.localeCompare(b.tag, undefined, { sensitivity: 'base' }) || compareCodePoints(a.tag, b.tag),
  );
}

// ── Finding things ──────────────────────────────────────────────────────────

/** The search text a tag chip puts in the search box. */
export const tagQuery = (tag) => `#${tag}`;

/**
 * Read a search query.
 *
 * A query that starts with `#` is a tag filter: `#tax` means "tagged Tax",
 * exactly, and `#tax #2026` means both. Tags may contain spaces, so a tag runs
 * until the next ` #`, which is also how a clicked chip ("#Insurance 2026")
 * reads back. Anything else is plain text matched against names and tags.
 *
 * @returns {{tags: string[]|null, text: string}} `tags` are lower-cased keys,
 *          null for a plain query.
 */
export function parseTagQuery(query) {
  const q = String(query ?? '').trim();
  if (!q.startsWith('#')) return { tags: null, text: q.toLowerCase() };
  const keys = q.split(/(?:^|\s)#+/)
    .map(normalizeTag)
    .filter(Boolean)
    .map(tagKey);
  return { tags: [...new Set(keys)], text: '' };
}

/**
 * Does a node match a parsed query? A lone `#` (still being typed) matches
 * anything that has a tag at all, which also makes it a quick "show me
 * everything tagged".
 */
export function matchesQuery(node, nodeTags, parsed) {
  const tags = nodeTags ?? [];
  if (parsed.tags) {
    const have = new Set(tags.map(tagKey));
    if (!parsed.tags.length) return have.size > 0;
    return parsed.tags.every((t) => have.has(t));
  }
  if (!parsed.text) return true;
  return String(node?.name ?? '').toLowerCase().includes(parsed.text)
    || tags.some((t) => tagKey(t).includes(parsed.text));
}

/** The nodes matching `query`, given each node's tags. */
export function searchNodes(nodes, tags, query) {
  const parsed = parseTagQuery(query);
  return (nodes ?? []).filter((n) => matchesQuery(n, tagsFor(tags, n.id), parsed));
}

// ── The reserved folder ─────────────────────────────────────────────────────
//
// `.mecloud` is MeCloud's own bookkeeping, not the user's files. It is hidden
// everywhere files are listed, but it is still real storage, so usage totals
// keep counting it.

/** The top-level `.mecloud` folder itself. */
export function isReservedFolder(node) {
  return Boolean(node) && !node.parentId && node.name === RESERVED_FOLDER && fileKind(node) === 'folder';
}

/** Ids of the reserved folder and everything below it. Cycle-safe. */
export function reservedIds(nodes) {
  const ids = new Set();
  const list = nodes ?? [];
  const roots = list.filter(isReservedFolder);
  if (!roots.length) return ids;

  const byParent = new Map();
  for (const node of list) {
    const key = node.parentId ?? null;
    if (!byParent.has(key)) byParent.set(key, []);
    byParent.get(key).push(node);
  }
  const walk = (id) => {
    if (ids.has(id)) return;
    ids.add(id);
    for (const child of byParent.get(id) ?? []) walk(child.id);
  };
  for (const root of roots) walk(root.id);
  return ids;
}

/** The nodes a user should see: everything except the reserved subtree. */
export function withoutReserved(nodes) {
  const hidden = reservedIds(nodes);
  return hidden.size ? (nodes ?? []).filter((n) => !hidden.has(n.id)) : (nodes ?? []);
}

/** Is this node the reserved folder or inside it? `nodesById` resolves parents. */
export function isReservedNode(node, nodesById) {
  let cur = node;
  const seen = new Set();
  while (cur && !seen.has(cur.id)) {
    if (isReservedFolder(cur)) return true;
    seen.add(cur.id);
    cur = cur.parentId ? nodesById?.get?.(cur.parentId) : null;
  }
  return false;
}

/** The reserved folder and the index file, as far as a listing knows. */
export function findTagIndexNodes(nodes) {
  const folder = (nodes ?? []).find(isReservedFolder) ?? null;
  const file = folder
    ? (nodes.find((n) => n.parentId === folder.id && n.name === TAG_INDEX_NAME && fileKind(n) !== 'folder') ?? null)
    : null;
  return { folder, file };
}

// ── Reading and writing over JMAP ───────────────────────────────────────────

/** Raised when tags cannot be changed (unreadable or newer index, or conflicts). */
export class TagIndexError extends Error {
  constructor(message, { readOnly = false } = {}) {
    super(message);
    this.name = 'TagIndexError';
    this.readOnly = readOnly;
  }
}

/**
 * The index file's contents, or MALFORMED for bytes that are not UTF-8.
 * Downloaded whole: the text preview helper truncates, and a truncated index
 * would read as malformed and lock tags for no reason.
 */
const MALFORMED = Symbol('malformed');
async function readIndexText(node) {
  if (!node?.blobId) return MALFORMED;
  const bytes = await fetchBlobBytes(node);
  try {
    // TextDecoder drops a leading BOM, so an index saved by an editor that
    // adds one still reads.
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  } catch {
    return MALFORMED;
  }
}

const parseText = (text) => (text === MALFORMED ? unreadableIndex(MALFORMED_MESSAGE) : parseIndex(text));

/**
 * Load the tag index.
 *
 * `nodes` is the listing the caller already has; the index is found in it, so
 * showing tags costs one download rather than another walk of the drive.
 *
 * @returns {Promise<{index: object, node: object|null, folder: object|null}>}
 */
export async function loadTagIndex(accountId, session, { nodes = null } = {}) {
  const listing = nodes ?? (await fetchFileNodes(accountId, session)).nodes;
  const { folder, file } = findTagIndexNodes(listing);
  if (!file) return { index: emptyIndex(), node: null, folder };
  return { index: parseText(await readIndexText(file)), node: file, folder };
}

async function getNodesWithState(accountId, session, ids) {
  const data = await jmapPost(
    [['FileNode/get', { accountId, ids }, 'g']],
    fileUsing(session),
  );
  if (!data) throw new Error('Your session expired.');
  const resp = data.methodResponses?.[0]?.[1] ?? {};
  return {
    state: resp.state ?? null,
    found: new Map((resp.list ?? []).map(normalizeNode).map((n) => [n.id, n])),
    notFound: new Set(resp.notFound ?? []),
  };
}

/**
 * Change tags: `change` is `{add, remove}` or `{set}` (see applyChange), for
 * every id in `nodeIds`.
 *
 * Read-modify-write, made safe by `ifInState`: the index is read together
 * with the drive's state, and the write only lands if nothing changed in
 * between. If something did — another tab, the desktop client — the server
 * says `stateMismatch` and the whole thing starts over from a fresh read, so
 * both changes survive. Five tries, then a clear error.
 *
 * `nodes`/`incomplete` are the caller's listing, used to find the index and
 * to prune entries for deleted nodes. A listing can be stale, so an entry is
 * only pruned once the server confirms that id is gone.
 *
 * @returns {Promise<{index: object, node: object|null, folder: object|null}>}
 */
export async function setTags(accountId, session, nodeIds, change = {}, { nodes = [], incomplete = true } = {}) {
  const listing = nodes ?? [];
  const listed = findTagIndexNodes(listing);
  let folderId = listed.folder?.id ?? null;
  let fileId = listed.file?.id ?? null;
  const listedIds = new Set(listing.map((n) => n.id));
  const targets = [...new Set(nodeIds ?? [])];
  const getCap = Math.max(1, maxObjectsInGet(session) ?? 256);
  // Ids the server has confirmed are gone. Checked once per call: a deleted
  // id stays deleted, so a retry does not need to ask again.
  let gone = null;

  let attempts = 0;
  let foldersCreated = 0;
  while (attempts < MAX_WRITE_ATTEMPTS) {
    // 1. The index node and the folder, read together with the drive's state.
    const { state, found } = await getNodesWithState(
      accountId, session, [...new Set([fileId, folderId].filter(Boolean))],
    );
    const fileNode = fileId ? (found.get(fileId) ?? null) : null;
    if (!fileNode) fileId = null;
    if (fileNode?.parentId) folderId = fileNode.parentId;
    else if (folderId && !found.has(folderId)) folderId = null;
    const folderNode = folderId
      ? (found.get(folderId) ?? { id: folderId, name: RESERVED_FOLDER, parentId: null, blobId: null })
      : null;

    // 2. Download and parse. Something we cannot read is never overwritten.
    const original = fileNode ? await readIndexText(fileNode) : null;
    const index = parseText(original);
    if (!index.writable) throw new TagIndexError(index.error, { readOnly: true });

    // 3. Apply the change.
    let next = applyChange(index, targets, change);

    // 4. Prune — only from a complete listing, and only ids the server
    //    confirms are gone, because the caller's listing may be older than a
    //    file another client has just added and tagged.
    if (!incomplete) {
      if (!gone) {
        const candidates = [...next.tags.keys()]
          .filter((id) => !listedIds.has(id) && !targets.includes(id))
          .slice(0, getCap);
        gone = candidates.length
          ? (await getNodesWithState(accountId, session, candidates)).notFound
          : new Set();
      }
      if (gone.size) next = prune(next, [...next.tags.keys()].filter((id) => !gone.has(id)));
    }

    const text = serializeIndex(next);
    if (fileNode && text === original) return { index: next, node: fileNode, folder: folderNode };
    if (!fileNode && next.tags.size === 0) return { index: next, node: null, folder: folderNode };

    // 5. Write, conditionally on nothing having changed since step 1.
    try {
      if (!folderId) {
        const created = await createFolder(accountId, session, RESERVED_FOLDER, null, { ifInState: state });
        folderId = created?.id ?? null;
        // Creating the folder moved the state on, so the file goes in on the
        // next pass from a fresh read. That is progress rather than a
        // conflict — unless it keeps happening.
        foldersCreated += 1;
        if (foldersCreated > 1) attempts += 1;
        continue;
      }

      const payload = new File([text], TAG_INDEX_NAME, { type: TAG_INDEX_TYPE });
      const blob = await uploadBlob(payload);
      const size = blob.size ?? payload.size;

      if (fileNode) {
        await updateNode(accountId, session, fileNode.id, { blobId: blob.blobId, size }, { ifInState: state });
        return {
          index: next,
          node: { ...fileNode, blobId: blob.blobId, size, modified: new Date().toISOString() },
          folder: folderNode,
        };
      }
      const created = await createFile(accountId, session, {
        name: TAG_INDEX_NAME,
        parentId: folderId,
        blobId: blob.blobId,
        type: blob.type || TAG_INDEX_TYPE,
        size,
      }, { ifInState: state });
      return { index: next, node: created, folder: folderNode };
    } catch (err) {
      // stateMismatch: something changed since the read. notFound: the index
      // was deleted under us. Either way, read again.
      if (err?.type === 'stateMismatch' || err?.type === 'notFound') {
        attempts += 1;
        continue;
      }
      if (err?.type === 'alreadyExists') {
        // Another client created the folder or the file first. Theirs is the
        // copy to merge into.
        attempts += 1;
        if (err.existingId) {
          if (!folderId) folderId = err.existingId; else fileId = err.existingId;
        } else {
          const fresh = findTagIndexNodes((await fetchFileNodes(accountId, session)).nodes);
          folderId = fresh.folder?.id ?? null;
          fileId = fresh.file?.id ?? null;
        }
        continue;
      }
      throw err;
    }
  }
  throw new TagIndexError(GAVE_UP_MESSAGE);
}
