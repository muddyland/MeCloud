/**
 * Notes, stored as Markdown files in JMAP file storage.
 *
 * There is no separate notes backend and no new data model: a note *is* a `.md`
 * FileNode under a folder called "Notes". That means the same documents are
 * reachable from the Files app, from WebDAV, and from any sync tool pointed at
 * the account — and an existing folder of Markdown files works untouched, which
 * is the point.
 */
import { getFileNodes, createFolder, createFile, destroyNodes, renameNode,
         fetchTextBlob, saveTextFile, uploadBlob } from './files.js';
import { fileKind, uniqueName } from './fileTypes.js';
import { isMarkdown } from './markdown.js';

/** The folder notes live in, at the root of the drive. */
export const NOTES_FOLDER = 'Notes';

const MD_EXTENSION = '.md';

/** Find the Notes folder among already-loaded nodes. Case-insensitive. */
export function findNotesFolder(nodes) {
  const wanted = NOTES_FOLDER.toLowerCase();
  return nodes.find(
    (n) => fileKind(n) === 'folder'
      && !n.parentId
      && String(n.name ?? '').toLowerCase() === wanted,
  ) ?? null;
}

/**
 * Every Markdown file at or below the Notes folder.
 *
 * Subfolders are included rather than ignored: an existing collection is very
 * likely organised into them, and silently showing only the top level would
 * look like data loss.
 */
export function collectNotes(nodes, folderId) {
  if (!folderId) return [];
  const byParent = new Map();
  for (const node of nodes) {
    const key = node.parentId ?? null;
    if (!byParent.has(key)) byParent.set(key, []);
    byParent.get(key).push(node);
  }

  const out = [];
  const seen = new Set();
  const walk = (parentId, trail) => {
    if (seen.has(parentId)) return;         // defensive against a parent cycle
    seen.add(parentId);
    for (const node of byParent.get(parentId) ?? []) {
      if (fileKind(node) === 'folder') {
        walk(node.id, [...trail, node.name]);
      } else if (isMarkdown(node)) {
        out.push({ ...node, folderPath: trail.join(' / ') });
      }
    }
  };
  walk(folderId, []);
  return out;
}

/** Load all file nodes and pick out the Notes folder and its Markdown files. */
export async function loadNotes(accountId, session) {
  const nodes = await getFileNodes(accountId, session);
  const folder = findNotesFolder(nodes);
  return { nodes, folder, notes: collectNotes(nodes, folder?.id) };
}

/** Create the Notes folder. Only called when it genuinely does not exist. */
export async function ensureNotesFolder(accountId, session, nodes) {
  const existing = findNotesFolder(nodes);
  if (existing) return { folder: existing, created: false };
  const folder = await createFolder(accountId, session, NOTES_FOLDER, null);
  return { folder, created: true };
}

/** Read a note's Markdown source. */
export function readNote(node) {
  return fetchTextBlob(node);
}

/** Write a note's Markdown source back. */
export function writeNote(accountId, session, node, text) {
  return saveTextFile(accountId, session, node, text, { type: 'text/markdown' });
}

/** Turn a title into a safe `.md` filename that does not collide. */
export function noteFilename(title, existingNames) {
  const cleaned = String(title ?? '').trim()
    // The FileNode spec forbids "/", and the rest are awkward across the
    // filesystems and sync tools these files will also be seen through.
    .replace(/[/\\:*?"<>|]/g, '-')
    .replace(/\s+/g, ' ')
    .slice(0, 120)
    .trim();
  const stem = cleaned || 'Untitled';
  return uniqueName(`${stem}${MD_EXTENSION}`, existingNames);
}

/** Create a new note file inside `folderId`. */
export async function createNote(accountId, session, folderId, { title = 'Untitled', body = '', existingNames = [] } = {}) {
  const name = noteFilename(title, existingNames);
  const initial = body || `# ${title}\n\n`;

  const payload = new File([initial], name, { type: 'text/markdown' });
  const blob = await uploadBlob(payload);

  const node = await createFile(accountId, session, {
    name,
    blobId: blob.blobId,
    type: blob.type || 'text/markdown',
    size: blob.size ?? payload.size,
    parentId: folderId,
  });
  return { node, text: initial };
}

export function renameNote(accountId, session, id, name) {
  const filename = /\.(md|markdown|mdown|mkd)$/i.test(name) ? name : `${name}${MD_EXTENSION}`;
  return renameNode(accountId, session, id, filename);
}

export function deleteNote(accountId, session, id) {
  return destroyNodes(accountId, session, [id], { recursive: false });
}
