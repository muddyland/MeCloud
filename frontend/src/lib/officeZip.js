/**
 * Shared plumbing for the Office previews: open a zip-based document
 * (.xlsx, .ods, .pptx, .odp) and read XML parts out of it safely.
 *
 * These files come from the drive and are untrusted. Two things keep a hostile
 * one from hurting the page:
 *  - every part is checked against its declared uncompressed size before it
 *    is inflated, so a few kilobytes cannot expand into gigabytes (a "zip
 *    bomb"), and the total read per document is capped too;
 *  - XML is parsed with DOMParser as data, and only text and attributes are
 *    read from it. Nothing from a document is ever inserted as HTML.
 */

export const MAX_OFFICE_BYTES = 20 * 1024 * 1024;     // the file itself
const MAX_PART_BYTES = 40 * 1024 * 1024;              // one part, inflated
const MAX_TOTAL_BYTES = 120 * 1024 * 1024;            // everything read, inflated

export class OfficeReadError extends Error {}

/** Open a zip document. `JSZip` is injectable for tests. */
export async function openZip(bytes, { JSZip } = {}) {
  const Zip = JSZip ?? (await import('jszip')).default;
  let zip;
  try {
    zip = await Zip.loadAsync(bytes);
  } catch {
    throw new OfficeReadError('This file is not a document this preview can read.');
  }
  return { zip, budget: { used: 0 } };
}

/** Normalise a zip path: no leading slash, "." and ".." resolved. */
export function normalisePath(path) {
  const out = [];
  for (const part of String(path ?? '').replace(/\\/g, '/').split('/')) {
    if (!part || part === '.') continue;
    if (part === '..') out.pop(); else out.push(part);
  }
  return out.join('/');
}

/** Resolve a relationship target against the part that names it. */
export function resolveTarget(fromPart, target) {
  if (String(target).startsWith('/')) return normalisePath(target);
  const dir = normalisePath(fromPart).split('/').slice(0, -1).join('/');
  return normalisePath(dir ? `${dir}/${target}` : target);
}

function declaredSize(file) {
  // JSZip keeps the central directory's sizes on the entry; reading them
  // costs nothing and needs no inflation.
  const size = file?._data?.uncompressedSize;
  return Number.isFinite(size) ? size : 0;
}

async function readPart(doc, path, type) {
  const file = doc.zip.file(normalisePath(path));
  if (!file) return null;
  const size = declaredSize(file);
  if (size > MAX_PART_BYTES || doc.budget.used + size > MAX_TOTAL_BYTES) {
    throw new OfficeReadError('This document is too large to preview. Download it instead.');
  }
  doc.budget.used += size;
  return file.async(type);
}

/** A part as text, or null when it does not exist. */
export function readText(doc, path) {
  return readPart(doc, path, 'string');
}

/** A part as bytes, or null when it does not exist. */
export function readBytes(doc, path) {
  return readPart(doc, path, 'uint8array');
}

/** A part parsed as XML, or null when missing or not well-formed. */
export async function readXml(doc, path) {
  const text = await readText(doc, path);
  if (text == null) return null;
  const xml = new DOMParser().parseFromString(text, 'application/xml');
  return xml.getElementsByTagName('parsererror').length ? null : xml;
}

/** Elements with this local name, whatever namespace prefix the file used. */
export function byName(node, localName) {
  return node ? [...node.getElementsByTagNameNS('*', localName)] : [];
}

/** Direct children with this local name. */
export function children(node, localName) {
  return node ? [...node.children].filter((c) => c.localName === localName) : [];
}

/** An attribute by local name, whatever its prefix. */
export function attr(node, localName) {
  if (!node?.attributes) return null;
  for (const a of node.attributes) if (a.localName === localName) return a.value;
  return null;
}

const REL_NS = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships';

/**
 * The relationship id an element points through (`r:id`). Read by namespace:
 * a slide entry also has a plain `id`, and taking the first attribute named
 * "id" picks the wrong one.
 */
export function relId(node) {
  return node?.getAttributeNS?.(REL_NS, 'id') || null;
}

/** Relationships of a part: Id -> { target (resolved), type, external }. */
export async function relationships(doc, partPath) {
  const norm = normalisePath(partPath);
  const dir = norm.split('/').slice(0, -1).join('/');
  const name = norm.split('/').pop();
  const relsPath = dir ? `${dir}/_rels/${name}.rels` : `_rels/${name}.rels`;
  const xml = await readXml(doc, relsPath);
  const out = new Map();
  for (const rel of byName(xml, 'Relationship')) {
    const external = attr(rel, 'TargetMode') === 'External';
    const target = attr(rel, 'Target') ?? '';
    out.set(attr(rel, 'Id'), {
      type: attr(rel, 'Type') ?? '',
      external,
      target: external ? target : resolveTarget(norm, target),
    });
  }
  return out;
}

/**
 * The media type of raster image bytes, sniffed from their content, or null.
 * Only PNG, JPEG, GIF and WebP: never SVG or anything else that could carry
 * script, whatever the file inside the zip claims to be.
 */
export function sniffRasterImage(bytes) {
  const b = bytes ?? [];
  if (b[0] === 0x89 && b[1] === 0x50 && b[2] === 0x4e && b[3] === 0x47) return 'image/png';
  if (b[0] === 0xff && b[1] === 0xd8 && b[2] === 0xff) return 'image/jpeg';
  if (b[0] === 0x47 && b[1] === 0x49 && b[2] === 0x46 && b[3] === 0x38) return 'image/gif';
  if (b[0] === 0x52 && b[1] === 0x49 && b[2] === 0x46 && b[3] === 0x46
    && b[8] === 0x57 && b[9] === 0x45 && b[10] === 0x42 && b[11] === 0x50) return 'image/webp';
  return null;
}
