/**
 * URL classification used by the email sanitiser.
 *
 * Kept separate from sanitize.js so it can be reasoned about — and tested —
 * without pulling DOMPurify (and therefore a DOM) into the module graph.
 */

/** Schemes that resolve without touching the network. */
const LOCAL_SCHEME = /^(data:|cid:|blob:|about:|#)/i;

/** Anything that would fetch from a host we don't control. */
const REMOTE = /^(https?:)?\/\//i;

export function isRemoteUrl(value) {
  if (!value) return false;
  const url = String(value).trim();
  if (!url || LOCAL_SCHEME.test(url)) return false;
  return REMOTE.test(url);
}

/**
 * True when a style attribute pulls a remote resource via `url(…)`.
 * CSS fetches just as effectively as markup does.
 */
export function hasRemoteCssUrl(style) {
  return /url\(\s*['"]?\s*(https?:)?\/\//i.test(String(style ?? ''));
}

/** Replace every remote `url(…)` in a style attribute with `none`. */
export function stripRemoteCssUrls(style) {
  return String(style ?? '').replace(/url\(\s*['"]?\s*(https?:)?\/\/[^)]*\)/gi, 'none');
}

/** Escape plain text for safe display inside a <pre>. */
export function escapeText(text) {
  return String(text ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

/**
 * Decode entities that some clients (notably iOS Mail) pre-encode into a
 * text/plain part, so `&gt;` renders as `>` rather than literally.
 */
export function decodeEntities(text) {
  return String(text ?? '')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#39;|&apos;/g, "'")
    .replace(/&amp;/g, '&');
}
