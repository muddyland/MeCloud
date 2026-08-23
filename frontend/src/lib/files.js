/**
 * JMAP for File Storage (urn:ietf:params:jmap:filenode).
 *
 * FileNode *metadata* is ordinary JMAP and rides the existing /api/jmap proxy.
 * Only the bytes need special handling: the bearer token lives in the encrypted
 * server-side session and never reaches the browser, so uploads and downloads
 * go through the backend's blob routes instead of straight at the JMAP
 * upload/download endpoints.
 */
import { jmapPost } from './api.js';
import { JmapSetError } from './jmapErrors.js';
import { begin, end } from './stores/activity.js';

export const FILENODE_CAPABILITY = 'urn:ietf:params:jmap:filenode';

/** Blob transfer endpoints, served by the backend. */
const BLOB_PATH = '/api/files/blob';

/** Upload retry policy for rate limiting. */
const MAX_UPLOAD_ATTEMPTS = 4;
const MAX_RETRY_DELAY_MS = 30_000;

/**
 * Parse a Retry-After header: either delta-seconds or an HTTP date.
 * Returns milliseconds, or null when absent or unparseable.
 */
export function parseRetryAfter(value, now = Date.now()) {
  if (value === null || value === undefined || value === '') return null;
  const raw = String(value).trim();

  if (/^\d+$/.test(raw)) return Number(raw) * 1000;

  const at = Date.parse(raw);
  if (Number.isNaN(at)) return null;
  return Math.max(0, at - now);
}

/**
 * How long to wait before retrying a rate-limited upload.
 *
 * Dropping a folder legitimately fires one request per file, so hitting the
 * per-minute limit is an expected outcome of normal use rather than abuse —
 * the client waits it out instead of failing the upload. The server's
 * Retry-After wins when present; otherwise exponential backoff with jitter so
 * a batch does not resume in lockstep.
 */
export function retryDelayMs(attempt, retryAfter, { random = Math.random, now = Date.now() } = {}) {
  const advised = parseRetryAfter(retryAfter, now);
  if (advised !== null) return Math.min(advised, MAX_RETRY_DELAY_MS);
  const backoff = Math.min(MAX_RETRY_DELAY_MS, 1000 * 2 ** attempt);
  return Math.round(backoff + random() * 500);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function fileUsing(session) {
  const caps = Object.keys(session?.capabilities ?? {});
  return caps.length ? caps : ['urn:ietf:params:jmap:core', FILENODE_CAPABILITY];
}

/** True when the connected server actually offers file storage. */
export function supportsFiles(session) {
  return Boolean(session?.capabilities?.[FILENODE_CAPABILITY]);
}

/** Server-advertised limits, with defensive defaults. */
export function fileLimits(session) {
  const cap = session?.capabilities?.[FILENODE_CAPABILITY] ?? {};
  return {
    maxDepth: cap.maxFileNodeDepth ?? null,
    maxNameLength: cap.maxSizeFileNodeName ?? 255,
    mayCreateTopLevel: cap.mayCreateTopLevelFileNode !== false,
    sortOptions: cap.fileNodeQuerySortOptions ?? [],
  };
}

// ── Reading ─────────────────────────────────────────────────────────────────

/**
 * Every node in the account, in one call.
 *
 * A Drive UI needs the whole tree anyway — breadcrumbs, the folder sidebar and
 * move targets all need ancestors that a single-folder listing would not
 * return. Fetching once and indexing in memory also makes navigation instant.
 * This matches how the app already loads Mailboxes and Calendars.
 */
const MAX_PAGES = 200;

/**
 * The cap on how many objects one /get may return, per RFC 8620 section 5.1.
 *
 * `ids: null` means "every record" only while the total stays under this
 * limit. Past it the server is supposed to answer `requestTooLarge`; Stalwart
 * silently truncates instead, which is worse — the listing looks complete and
 * is not, so a node past the cut is invisible while still blocking its name.
 */
export function maxObjectsInGet(session) {
  const n = session?.capabilities?.['urn:ietf:params:jmap:core']?.maxObjectsInGet;
  return Number.isFinite(n) ? n : null;
}

/**
 * Page through the whole tree with query + a back-reference to get.
 *
 * This is the enumeration RFC 8620 actually provides for a set larger than one
 * call can carry: /query pages by position, and the result reference feeds
 * those ids straight into /get so each page costs one round trip.
 */
async function queryAllNodes(accountId, session, pageSize) {
  const nodes = [];
  const seen = new Set();
  let position = 0;

  for (let page = 0; page < MAX_PAGES; page += 1) {
    const data = await jmapPost(
      [
        ['FileNode/query', { accountId, position, limit: pageSize }, 'q'],
        ['FileNode/get', {
          accountId,
          '#ids': { resultOf: 'q', name: 'FileNode/query', path: '/ids' },
        }, 'g'],
      ],
      fileUsing(session),
    );

    const ids = data?.methodResponses?.[0]?.[1]?.ids ?? [];
    if (!ids.length) break;

    let added = 0;
    for (const node of data?.methodResponses?.[1]?.[1]?.list ?? []) {
      if (seen.has(node.id)) continue;
      seen.add(node.id);
      nodes.push(node);
      added += 1;
    }
    // A server that ignores `position` would otherwise return page one forever.
    if (added === 0) break;

    // Advance by what the server actually returned, not by what was asked for:
    // it is free to cap `limit` below pageSize, and stepping by pageSize would
    // then skip everything in between.
    position += ids.length;
  }

  return nodes;
}

/**
 * Every node in the account, and whether that is actually all of them.
 *
 * @returns {Promise<{nodes: object[], incomplete: boolean}>}
 */
export async function fetchFileNodes(accountId, session) {
  const pageSize = maxObjectsInGet(session) ?? 256;

  try {
    return { nodes: await queryAllNodes(accountId, session, pageSize), incomplete: false };
  } catch {
    // FileNode/query is newer than FileNode/get and a server may not have it.
    // Falling back keeps a small drive working rather than failing outright,
    // but a fallback landing on the ceiling is the truncation case and says so
    // instead of pretending the listing is whole.
    const data = await jmapPost(
      [['FileNode/get', { accountId, ids: null }, 'f']],
      fileUsing(session),
    );
    const nodes = data?.methodResponses?.[0]?.[1]?.list ?? [];
    return { nodes, incomplete: nodes.length >= pageSize };
  }
}

/** Every node in the account, for callers that cannot act on incompleteness. */
export async function getFileNodes(accountId, session) {
  const { nodes } = await fetchFileNodes(accountId, session);
  return nodes;
}

// ── Mutating ────────────────────────────────────────────────────────────────

function firstSetError(resp, key) {
  if (resp?.notCreated?.[key]) return resp.notCreated[key];
  if (resp?.notUpdated?.[key]) return resp.notUpdated[key];
  if (resp?.notDestroyed?.[key]) return resp.notDestroyed[key];
  return null;
}

/**
 * Interpret a `/set` response for a single creation id.
 *
 * The distinction this draws is the one that matters, and getting it wrong
 * broke folder creation outright: a `notCreated` entry means the server
 * refused, while the *absence* of a `created` entry means only that it did not
 * echo the new object back. Treating the second as a failure rejects writes
 * that actually succeeded.
 *
 * @returns {{ node: object|null, error: object|null }}
 */
export function interpretCreate(resp, key) {
  const error = firstSetError(resp, key);
  if (error) return { node: null, error };
  const created = resp?.created?.[key];
  return { node: created?.id ? created : null, error: null };
}

export async function createFolder(accountId, session, name, parentId = null) {
  const data = await jmapPost(
    [['FileNode/set', { accountId, create: { nf: { name, parentId } } }, 'f']],
    fileUsing(session),
  );
  const { node, error } = interpretCreate(data?.methodResponses?.[0]?.[1], 'nf');
  if (error) throw new JmapSetError(error, 'Could not create the folder.');
  return node ? { name, parentId, blobId: null, ...node } : null;
}

/** Attach an already-uploaded blob to a new FileNode. */
export async function createFile(accountId, session, { name, blobId, type, size, parentId = null }) {
  const data = await jmapPost(
    [['FileNode/set', {
      accountId,
      create: { nn: { name, parentId, blobId, type, size } },
    }, 'f']],
    fileUsing(session),
  );
  const { node, error } = interpretCreate(data?.methodResponses?.[0]?.[1], 'nn');
  if (error) throw new JmapSetError(error, 'Could not save the file.');
  return node ? { name, parentId, blobId, type, size, ...node } : null;
}

export async function updateNode(accountId, session, id, patch) {
  const data = await jmapPost(
    [['FileNode/set', { accountId, update: { [id]: patch } }, 'f']],
    fileUsing(session),
  );
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notUpdated?.[id]) {
    throw new JmapSetError(resp.notUpdated[id], 'Could not update the item.');
  }
}

export const renameNode = (accountId, session, id, name) =>
  updateNode(accountId, session, id, { name });

export const moveNode = (accountId, session, id, parentId) =>
  updateNode(accountId, session, id, { parentId });

/**
 * Destroy nodes. `onDestroyRemoveChildren` is required for folders — without it
 * the server refuses to delete anything that still has contents.
 */
export async function destroyNodes(accountId, session, ids, { recursive = true } = {}) {
  const data = await jmapPost(
    [['FileNode/set', {
      accountId,
      destroy: ids,
      onDestroyRemoveChildren: recursive,
    }, 'f']],
    fileUsing(session),
  );
  const resp = data?.methodResponses?.[0]?.[1];
  const failed = resp?.notDestroyed ? Object.keys(resp.notDestroyed) : [];
  if (failed.length) {
    const first = resp.notDestroyed[failed[0]];
    throw new JmapSetError(first, `Could not delete ${failed.length} item(s).`);
  }
  return resp?.destroyed ?? ids;
}

// ── Blob transfer ───────────────────────────────────────────────────────────

/**
 * URL for a blob, via the backend proxy.
 *
 * `inline` is only a request — the backend keeps its own allow-list and will
 * force a download for anything that could execute as script on our origin.
 */
export function blobUrl(node, { inline = false } = {}) {
  if (!node?.blobId) return '';
  const params = new URLSearchParams({
    name: node.name ?? 'download',
    type: node.type ?? '',
  });
  if (inline) params.set('inline', 'true');
  return `${BLOB_PATH}/${encodeURIComponent(node.blobId)}?${params}`;
}

/**
 * Upload one file, reporting progress.
 *
 * Deliberately XMLHttpRequest rather than fetch: fetch still gives no upload
 * progress events, and a Drive UI without a progress bar on a 50 MB upload
 * feels broken.
 *
 * @returns {Promise<{blobId: string, type: string, size: number}>}
 */
function uploadBlobOnce(file, { onProgress, signal } = {}) {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    begin();

    const settle = (fn) => (...args) => { end(); fn(...args); };
    const done = settle(resolve);
    const fail = settle(reject);

    xhr.open('POST', BLOB_PATH, true);
    xhr.withCredentials = true;
    xhr.setRequestHeader('Content-Type', file.type || 'application/octet-stream');

    if (onProgress) {
      xhr.upload.onprogress = (e) => {
        if (e.lengthComputable) onProgress(e.loaded / e.total);
      };
    }

    xhr.onload = () => {
      if (xhr.status === 401) {
        window.location.href = '/auth/login';
        fail(new Error('Your session expired.'));
        return;
      }
      if (xhr.status === 413) {
        fail(new Error(`"${file.name}" is too large to upload.`));
        return;
      }
      if (xhr.status === 429) {
        // Surface the server's advice so the caller can wait exactly as long
        // as it asked, rather than guessing.
        const err = new Error('Rate limited');
        err.rateLimited = true;
        err.retryAfter = xhr.getResponseHeader('Retry-After');
        fail(err);
        return;
      }
      if (xhr.status < 200 || xhr.status >= 300) {
        let detail = `Upload failed (${xhr.status})`;
        try {
          detail = JSON.parse(xhr.responseText)?.detail ?? detail;
        } catch {
          // Non-JSON error body; keep the status-based message.
        }
        fail(new Error(detail));
        return;
      }
      try {
        done(JSON.parse(xhr.responseText));
      } catch {
        fail(new Error('The server returned an unreadable response.'));
      }
    };

    xhr.onerror = () => fail(new Error('The upload could not reach the server.'));
    xhr.onabort = () => fail(new DOMException('Upload cancelled', 'AbortError'));

    if (signal) {
      if (signal.aborted) { xhr.abort(); return; }
      signal.addEventListener('abort', () => xhr.abort(), { once: true });
    }

    xhr.send(file);
  });
}

/**
 * Upload one blob, waiting out rate limits rather than failing on them.
 *
 * @param {object} [opts.onWait] called with (ms, attempt) while backing off, so
 *        the UI can say "waiting" instead of appearing to stall.
 */
export async function uploadBlob(file, { onProgress, signal, onWait } = {}) {
  for (let attempt = 0; ; attempt += 1) {
    try {
      return await uploadBlobOnce(file, { onProgress, signal });
    } catch (err) {
      const canRetry = err?.rateLimited && attempt < MAX_UPLOAD_ATTEMPTS - 1;
      if (!canRetry) {
        if (err?.rateLimited) {
          throw new Error('The server is rate limiting uploads — try again shortly.');
        }
        throw err;
      }
      const delay = retryDelayMs(attempt, err.retryAfter);
      onWait?.(delay, attempt + 1);
      await sleep(delay);
      if (signal?.aborted) throw new DOMException('Upload cancelled', 'AbortError');
    }
  }
}

/** Upload the bytes, then create the FileNode that points at them. */
export async function uploadFile(accountId, session, file, { parentId = null, name, onProgress, onWait, signal } = {}) {
  const blob = await uploadBlob(file, { onProgress, onWait, signal });
  return createFile(accountId, session, {
    name: name ?? file.name,
    blobId: blob.blobId,
    type: blob.type || file.type || 'application/octet-stream',
    size: blob.size ?? file.size,
    parentId,
  });
}

/**
 * Replace a file's contents with `text`, keeping the same FileNode.
 *
 * Two steps, because JMAP separates bytes from metadata: upload a new blob,
 * then repoint the node at it. The node id is stable across saves, so anything
 * holding a reference (an open editor, the notes list) stays valid.
 *
 * Shared by the Notes app and the Files preview's edit mode — both are "save
 * this text back over that node", and having one implementation means the
 * blob/metadata ordering only has to be right once.
 */
export async function saveTextFile(accountId, session, node, text, { type } = {}) {
  if (!node?.id) throw new Error('Cannot save: this item has no id.');
  const mediaType = type || node.type || 'text/markdown';

  // A File rather than a Blob: the upload path reads `.name`, and the server
  // is happier with a filename attached.
  const payload = new File([text ?? ''], node.name ?? 'note.md', { type: mediaType });
  const blob = await uploadBlob(payload);

  await updateNode(accountId, session, node.id, {
    blobId: blob.blobId,
    size: blob.size ?? payload.size,
    type: blob.type || mediaType,
  });

  return {
    ...node,
    blobId: blob.blobId,
    size: blob.size ?? payload.size,
    type: blob.type || mediaType,
    modified: new Date().toISOString(),
  };
}

/** Fetch a text blob's contents, for the inline preview. */
export async function fetchTextBlob(node, { maxBytes = 512 * 1024 } = {}) {
  const res = await fetch(blobUrl(node, { inline: true }), { credentials: 'include' });
  if (!res.ok) throw new Error('Could not load a preview of this file.');
  const text = await res.text();
  return text.length > maxBytes ? `${text.slice(0, maxBytes)}\n\n… truncated` : text;
}

/** Trigger a browser download without navigating away from the app. */
export function downloadNode(node) {
  const url = blobUrl(node, { inline: false });
  if (!url) return;
  const a = document.createElement('a');
  a.href = url;
  a.download = node.name ?? 'download';
  a.rel = 'noopener';
  document.body.appendChild(a);
  a.click();
  a.remove();
}

// ── Dropped directories ─────────────────────────────────────────────────────
//
// `DataTransfer.files` flattens a dropped folder into a single zero-byte entry
// with no type — the contents are simply not there. Only the entries API
// (`webkitGetAsEntry`) exposes the tree, and it has two sharp edges:
//
//   1. It must be read *synchronously* during the drop event. The DataTransfer
//      is neutered as soon as the handler yields, so awaiting first loses it.
//   2. `readEntries()` returns at most 100 children per call and must be
//      drained in a loop — the usual cause of "only some files uploaded".

/** Depth and count ceilings, so a pathological drop cannot hang the tab. */
const MAX_TREE_DEPTH = 16;
const MAX_TREE_FILES = 2000;

/**
 * Synchronously extract FileSystemEntry objects from a drop.
 * Returns null when the browser has no entries API, so the caller can fall
 * back to the flat `files` list.
 */
export function readDropEntries(dataTransfer) {
  const items = dataTransfer?.items;
  if (!items?.length) return null;

  const entries = [];
  for (const item of Array.from(items)) {
    if (item.kind !== 'file') continue;
    const getEntry = item.webkitGetAsEntry ?? item.getAsEntry;
    if (typeof getEntry !== 'function') return null;
    const entry = getEntry.call(item);
    if (entry) entries.push(entry);
  }
  return entries.length ? entries : null;
}

function readAllChildren(reader) {
  // readEntries yields in batches and signals completion with an empty batch.
  return new Promise((resolve, reject) => {
    const all = [];
    const step = () => reader.readEntries((batch) => {
      if (!batch.length) { resolve(all); return; }
      all.push(...batch);
      step();
    }, reject);
    step();
  });
}

const entryFile = (entry) =>
  new Promise((resolve, reject) => entry.file(resolve, reject));

/**
 * Turn dropped entries into a tree of
 * `{ kind: 'file', name, file } | { kind: 'dir', name, children }`.
 */
export async function buildDropTree(entries, { depth = 0, budget = { files: 0 } } = {}) {
  const nodes = [];
  for (const entry of entries) {
    if (entry.isFile) {
      if (budget.files >= MAX_TREE_FILES) break;
      budget.files += 1;
      try {
        nodes.push({ kind: 'file', name: entry.name, file: await entryFile(entry) });
      } catch {
        // Unreadable entry (permissions, a vanished file) — skip it rather than
        // failing the whole drop.
      }
    } else if (entry.isDirectory && depth < MAX_TREE_DEPTH) {
      const children = await readAllChildren(entry.createReader());
      nodes.push({
        kind: 'dir',
        name: entry.name,
        children: await buildDropTree(children, { depth: depth + 1, budget }),
      });
    }
  }
  return nodes;
}

/** Count the files in a drop tree, for progress reporting. */
export function countTreeFiles(nodes) {
  return nodes.reduce(
    (n, node) => n + (node.kind === 'file' ? 1 : countTreeFiles(node.children ?? [])),
    0,
  );
}
