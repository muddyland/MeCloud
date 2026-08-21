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
import { begin, end } from './stores/activity.js';

export const FILENODE_CAPABILITY = 'urn:ietf:params:jmap:filenode';

/** Blob transfer endpoints, served by the backend. */
const BLOB_PATH = '/api/files/blob';

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
export async function getFileNodes(accountId, session) {
  const data = await jmapPost(
    [['FileNode/get', { accountId, ids: null }, 'f']],
    fileUsing(session),
  );
  return data?.methodResponses?.[0]?.[1]?.list ?? [];
}

// ── Mutating ────────────────────────────────────────────────────────────────

function firstSetError(resp, key) {
  if (resp?.notCreated?.[key]) return resp.notCreated[key];
  if (resp?.notUpdated?.[key]) return resp.notUpdated[key];
  if (resp?.notDestroyed?.[key]) return resp.notDestroyed[key];
  return null;
}

export async function createFolder(accountId, session, name, parentId = null) {
  const data = await jmapPost(
    [['FileNode/set', { accountId, create: { nf: { name, parentId } } }, 'f']],
    fileUsing(session),
  );
  const resp = data?.methodResponses?.[0]?.[1];
  const err = firstSetError(resp, 'nf');
  if (err) throw new Error(err.description || 'Could not create the folder.');
  return { name, parentId, blobId: null, ...(resp?.created?.nf ?? {}) };
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
  const resp = data?.methodResponses?.[0]?.[1];
  const err = firstSetError(resp, 'nn');
  if (err) throw new Error(err.description || 'Could not save the file.');
  return { name, parentId, blobId, type, size, ...(resp?.created?.nn ?? {}) };
}

export async function updateNode(accountId, session, id, patch) {
  const data = await jmapPost(
    [['FileNode/set', { accountId, update: { [id]: patch } }, 'f']],
    fileUsing(session),
  );
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notUpdated?.[id]) {
    throw new Error(resp.notUpdated[id].description || 'Could not update the item.');
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
    throw new Error(first?.description || `Could not delete ${failed.length} item(s).`);
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
export function uploadBlob(file, { onProgress, signal } = {}) {
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

/** Upload the bytes, then create the FileNode that points at them. */
export async function uploadFile(accountId, session, file, { parentId = null, name, onProgress, signal } = {}) {
  const blob = await uploadBlob(file, { onProgress, signal });
  return createFile(accountId, session, {
    name: name ?? file.name,
    blobId: blob.blobId,
    type: blob.type || file.type || 'application/octet-stream',
    size: blob.size ?? file.size,
    parentId,
  });
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
