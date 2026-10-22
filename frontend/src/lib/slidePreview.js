/**
 * Read-only presentation previews: .pptx/.ppsx/.potx/.pptm and .odp.
 *
 * No library renders slides faithfully in a browser, so this does not try.
 * It shows what can be shown truthfully: the cover thumbnail most files carry
 * (usually the first slide, saved by the app that wrote the file) and an
 * outline of every slide — its title and its text. Both are rendered as an
 * image and plain text; nothing from the file is treated as HTML, and the
 * thumbnail is only used if its bytes really are a raster image.
 */
import {
  attr, byName, openZip, relId, readBytes, readXml, relationships,
  sniffRasterImage, OfficeReadError,
} from './officeZip.js';

const MAX_SLIDES = 300;
const MAX_LINES = 12;
const THUMBNAIL_TYPE = 'http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail';

/**
 * @typedef {{ number: number, title: string, lines: string[], hidden?: boolean }} Slide
 * @returns {Promise<{ thumbnail: Blob|null, slides: Slide[], truncated: boolean }>}
 */
export async function readPresentation(bytes, name, deps = {}) {
  const doc = await openZip(bytes, deps);
  if (doc.zip.file('content.xml') && !doc.zip.file('ppt/presentation.xml')) return readOdp(doc);
  return readPptx(doc);
}

async function thumbnailFrom(doc, path) {
  if (!path) return null;
  const bytes = await readBytes(doc, path);
  const type = sniffRasterImage(bytes);
  return type ? new Blob([bytes], { type }) : null;
}

/** Lines of text, trimmed, blank ones dropped, capped. */
function tidy(lines) {
  return lines.map((l) => l.replace(/\s+/g, ' ').trim()).filter(Boolean);
}

// ── .pptx ───────────────────────────────────────────────────────────────────

/** The text of one DrawingML paragraph: its runs and fields, breaks as spaces. */
function paragraphText(p) {
  let out = '';
  for (const node of p.getElementsByTagNameNS('*', '*')) {
    if (node.localName === 't') out += node.textContent;
    else if (node.localName === 'br') out += ' ';
  }
  return out;
}

const TITLE_PLACEHOLDERS = new Set(['title', 'ctrTitle']);

function slideOutline(xml) {
  let title = '';
  const lines = [];
  // Shapes in document order; a title placeholder names the slide.
  for (const shape of byName(xml, 'sp')) {
    const ph = byName(shape, 'ph')[0];
    const paras = tidy(byName(shape, 'p').filter((p) => p.namespaceURI?.includes('drawingml')).map(paragraphText));
    if (!paras.length) continue;
    if (!title && ph && TITLE_PLACEHOLDERS.has(attr(ph, 'type'))) title = paras.join(' ');
    else lines.push(...paras);
  }
  // Text in tables and other graphic frames.
  for (const frame of byName(xml, 'graphicFrame')) {
    lines.push(...tidy(byName(frame, 'p').map(paragraphText)));
  }
  if (!title && lines.length) title = lines.shift();
  return { title, lines };
}

async function readPptx(doc) {
  const presentation = await readXml(doc, 'ppt/presentation.xml');
  if (!presentation) throw new OfficeReadError('This presentation could not be read.');
  const rels = await relationships(doc, 'ppt/presentation.xml');
  const rootRels = await relationships(doc, '_rels/.rels');
  const thumbRel = [...rootRels.values()].find((r) => r.type === THUMBNAIL_TYPE && !r.external);

  const ids = byName(presentation, 'sldId');
  const slides = [];
  for (const sldId of ids.slice(0, MAX_SLIDES)) {
    const rel = rels.get(relId(sldId));
    if (!rel || rel.external) continue;
    const xml = await readXml(doc, rel.target);
    if (!xml) continue;
    const { title, lines } = slideOutline(xml);
    slides.push({
      number: slides.length + 1,
      title,
      lines: lines.slice(0, MAX_LINES),
      hidden: attr(xml.documentElement, 'show') === '0',
    });
  }
  if (!slides.length) throw new OfficeReadError('This presentation has no slides to show.');
  return {
    thumbnail: await thumbnailFrom(doc, thumbRel?.target ?? 'docProps/thumbnail.jpeg'),
    slides,
    truncated: ids.length > MAX_SLIDES,
  };
}

// ── .odp ────────────────────────────────────────────────────────────────────

function odfParagraphs(node) {
  return tidy([...byName(node, 'p'), ...byName(node, 'h')]
    .filter((p) => p.namespaceURI?.includes(':text:'))
    .map((p) => p.textContent));
}

function insideNotes(node, stop) {
  for (let p = node.parentNode; p && p !== stop; p = p.parentNode) {
    if (p.localName === 'notes') return true;
  }
  return false;
}

async function readOdp(doc) {
  const content = await readXml(doc, 'content.xml');
  if (!content) throw new OfficeReadError('This presentation could not be read.');
  const pages = byName(content, 'page').filter((p) => p.namespaceURI?.includes(':drawing:'));
  const slides = [];
  for (const page of pages.slice(0, MAX_SLIDES)) {
    let title = '';
    const lines = [];
    for (const frame of byName(page, 'frame')) {
      // Speaker notes live in a nested <presentation:notes>; not slide text.
      if (insideNotes(frame, page)) continue;
      const paras = odfParagraphs(frame);
      if (!paras.length) continue;
      if (!title && attr(frame, 'class') === 'title') title = paras.join(' ');
      else lines.push(...paras);
    }
    if (!title && lines.length) title = lines.shift();
    slides.push({ number: slides.length + 1, title, lines: lines.slice(0, MAX_LINES) });
  }
  if (!slides.length) throw new OfficeReadError('This presentation has no slides to show.');
  return {
    thumbnail: await thumbnailFrom(doc, 'Thumbnails/thumbnail.png'),
    slides,
    truncated: pages.length > MAX_SLIDES,
  };
}
