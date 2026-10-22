/**
 * Read-only previews of Word (.docx) files.
 *
 * A .docx is untrusted input from the drive, so it is handled the way mail is:
 *
 *  1. docx-preview turns it into HTML in a detached element. It only ever
 *     reads images and fonts from inside the file itself (as data: URLs), so
 *     rendering makes no network requests. Embedded HTML chunks — the one
 *     feature that carries arbitrary markup — are switched off.
 *  2. The result goes through the mail sanitiser: no script, no event
 *     handlers, no javascript: links, and no remote references in CSS.
 *  3. It is shown in a sandboxed iframe with no allow-scripts and no
 *     allow-same-origin, exactly like a message body. That frame is the real
 *     boundary; the sanitiser is defence in depth.
 *
 * The renderer is loaded on first use, so it costs nothing until a Word file
 * is actually opened.
 */
import { sanitizeEmailHtml } from './sanitize.js';

// Rendering is synchronous work on the main thread and the whole file is held
// in memory; past this a download is the better experience.
export const MAX_DOCX_BYTES = 20 * 1024 * 1024;

const RENDER_OPTIONS = {
  inWrapper: true,                 // grey backdrop with white pages, like Word
  breakPages: true,
  ignoreLastRenderedPageBreak: true,
  useBase64URL: true,              // data: URLs, which survive the sandboxed frame
  renderAltChunks: false,          // embedded HTML: never
  renderComments: false,
  renderChanges: false,
  experimental: false,
};

// The frame's own page chrome: let the document's wrapper fill it.
const FRAME_CSS = 'html,body{margin:0;padding:0;background:#e5e7eb}';

/**
 * Render .docx bytes to a complete, sanitised HTML document for an iframe's
 * srcdoc. Throws if the file is not a readable .docx.
 *
 * @param {ArrayBuffer|Blob} data
 * @param {{ renderer?: { renderAsync: Function } }} [deps] for tests
 */
export async function renderDocxDocument(data, { renderer } = {}) {
  const { renderAsync } = renderer ?? await import('docx-preview');
  const body = document.createElement('div');
  const styles = document.createElement('div');
  await renderAsync(data, body, styles, RENDER_OPTIONS);
  const raw = '<!DOCTYPE html><html><head>'
    + styles.innerHTML
    + `<style>${FRAME_CSS}</style>`
    + '</head><body>' + body.innerHTML + '</body></html>';
  return sanitizeEmailHtml(raw, { wholeDocument: true }).html;
}
