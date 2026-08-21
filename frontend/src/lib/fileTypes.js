/**
 * File classification helpers.
 *
 * Pure and dependency-free so the Files UI, the mail attachment list and the
 * unit tests all agree on what a given file *is* — the kind drives the icon,
 * the accent colour and whether we are willing to preview it inline.
 */

/** Human-readable size. `null`/undefined render as an em dash rather than "0 B". */
export function formatBytes(bytes) {
  if (bytes === null || bytes === undefined || bytes === '') return '—';
  const n = Number(bytes);
  if (!Number.isFinite(n) || n < 0) return '—';
  if (n === 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let value = n;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const rounded = value < 10 && unit > 0 ? value.toFixed(1) : Math.round(value);
  return `${rounded} ${units[unit]}`;
}

/** Lowercase extension without the dot, or '' when there isn't one. */
export function extensionOf(name) {
  const base = String(name ?? '').split('/').pop();
  const dot = base.lastIndexOf('.');
  if (dot <= 0 || dot === base.length - 1) return '';
  return base.slice(dot + 1).toLowerCase();
}

const BY_EXTENSION = {
  pdf: 'pdf',
  doc: 'doc', docx: 'doc', odt: 'doc', rtf: 'doc', pages: 'doc',
  xls: 'sheet', xlsx: 'sheet', ods: 'sheet', csv: 'sheet', numbers: 'sheet',
  ppt: 'slides', pptx: 'slides', odp: 'slides', key: 'slides',
  txt: 'text', md: 'text', log: 'text',
  js: 'code', mjs: 'code', ts: 'code', jsx: 'code', tsx: 'code', json: 'code',
  py: 'code', rb: 'code', go: 'code', rs: 'code', java: 'code', c: 'code',
  h: 'code', cpp: 'code', cs: 'code', sh: 'code', yml: 'code', yaml: 'code',
  toml: 'code', html: 'code', css: 'code', svelte: 'code', vue: 'code', sql: 'code',
  zip: 'archive', tar: 'archive', gz: 'archive', tgz: 'archive', bz2: 'archive',
  xz: 'archive', '7z': 'archive', rar: 'archive',
};

/**
 * A coarse kind: 'folder' | 'image' | 'video' | 'audio' | 'pdf' | 'doc' |
 * 'sheet' | 'slides' | 'text' | 'code' | 'archive' | 'file'.
 *
 * Media type wins when present — it comes from the server. Extension is the
 * fallback, because plenty of uploads arrive as application/octet-stream.
 */
export function fileKind(node) {
  if (!node) return 'file';
  // Per the FileNode spec a folder is exactly "blobId is null".
  if (node.blobId === null || node.blobId === undefined) return 'folder';

  const type = String(node.type ?? '').toLowerCase();
  if (type.startsWith('image/')) return 'image';
  if (type.startsWith('video/')) return 'video';
  if (type.startsWith('audio/')) return 'audio';
  if (type === 'application/pdf') return 'pdf';

  const byExt = BY_EXTENSION[extensionOf(node.name)];
  if (byExt) return byExt;

  if (type.startsWith('text/')) return 'text';
  return 'file';
}

// Only these are ever requested with inline=true. The backend independently
// enforces its own allow-list — this just avoids asking for a preview we know
// will come back as a download anyway.
const PREVIEWABLE = new Set(['image', 'pdf', 'text', 'video', 'audio']);

export function isPreviewable(node) {
  return PREVIEWABLE.has(fileKind(node));
}

// Control characters are rejected outright: they are invisible in the UI and
// have no business in a filename.
const CONTROL_CHARS = /[\u0000-\u001f\u007f]/;

/**
 * Validate a proposed file or folder name.
 * The FileNode spec forbids "/" and "."; the rest is defensive.
 * @returns {string} an error message, or '' when the name is acceptable
 */
export function validateName(name, { maxLength = 255 } = {}) {
  const trimmed = String(name ?? '').trim();
  if (!trimmed) return 'Name cannot be empty.';
  if (trimmed === '.' || trimmed === '..') return 'That name is reserved.';
  if (trimmed.includes('/')) return 'Names cannot contain "/".';
  if (CONTROL_CHARS.test(trimmed)) return 'Names cannot contain control characters.';
  if (trimmed.length > maxLength) return `Names must be ${maxLength} characters or fewer.`;
  return '';
}

/** Walk parentIds to build a root-first breadcrumb trail for `nodeId`. */
export function breadcrumbTrail(nodesById, nodeId) {
  const trail = [];
  const seen = new Set();
  let current = nodeId;
  while (current) {
    if (seen.has(current)) break;      // defensive: a cycle would hang the UI
    seen.add(current);
    const node = nodesById.get ? nodesById.get(current) : nodesById[current];
    if (!node) break;
    trail.unshift(node);
    current = node.parentId;
  }
  return trail;
}

/** Comparator for the listing. Folders always lead, as in every file manager. */
export function compareNodes(a, b, { key = 'name', ascending = true } = {}) {
  const aFolder = fileKind(a) === 'folder';
  const bFolder = fileKind(b) === 'folder';
  if (aFolder !== bFolder) return aFolder ? -1 : 1;

  let result;
  if (key === 'size') {
    result = (Number(a.size) || 0) - (Number(b.size) || 0);
  } else if (key === 'modified') {
    result = new Date(a.modified ?? 0) - new Date(b.modified ?? 0);
  } else {
    result = String(a.name ?? '').localeCompare(String(b.name ?? ''), undefined,
      { numeric: true, sensitivity: 'base' });
  }
  return ascending ? result : -result;
}

/** Disambiguate a name against those already present: "report (2).pdf". */
export function uniqueName(name, existingNames) {
  const taken = existingNames instanceof Set ? existingNames : new Set(existingNames ?? []);
  if (!taken.has(name)) return name;

  const ext = extensionOf(name);
  const stem = ext ? name.slice(0, -(ext.length + 1)) : name;
  for (let i = 2; i < 1000; i += 1) {
    const candidate = ext ? `${stem} (${i}).${ext}` : `${stem} (${i})`;
    if (!taken.has(candidate)) return candidate;
  }
  return `${stem}-x${ext ? `.${ext}` : ''}`;
}
