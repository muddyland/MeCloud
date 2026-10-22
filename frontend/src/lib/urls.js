/**
 * URL classification used by the email sanitiser.
 *
 * Kept separate from sanitize.js so it can be reasoned about — and tested —
 * without pulling DOMPurify (and therefore a DOM) into the module graph.
 */

/** Schemes that resolve without touching the network. */
const LOCAL_SCHEME = /^(data:|cid:|blob:|about:|#)/i;

/**
 * Anything that would fetch from a host we don't control.
 *
 * Two shapes, both following the WHATWG URL parser rather than what looks
 * plausible: an http(s) scheme with *any* number of slashes (`https:evil.test`
 * is parsed as `https://evil.test/` for special schemes), and a scheme-relative
 * reference, where a backslash counts as a slash (`/\evil.test`).
 */
const REMOTE = /^(https?:|[\\/]{2})/i;

/** The parser strips ASCII tab and newline from anywhere in a URL. */
const URL_INVISIBLE = /[\t\n\r]/g;

export function isRemoteUrl(value) {
  if (!value) return false;
  // Leading/trailing C0 controls and spaces are stripped by the URL parser too.
  // eslint-disable-next-line no-control-regex
  const url = String(value).replace(URL_INVISIBLE, '').replace(/^[\u0000-\u0020]+|[\u0000-\u0020]+$/g, '');
  if (!url || LOCAL_SCHEME.test(url)) return false;
  return REMOTE.test(url);
}

/** Characters an escape may decode to that would change the token structure. */
const inert = (ch) => (/["'()\\]/.test(ch) ? '_' : ch);

/**
 * CSS with comments removed and escapes decoded, tokenised the way a browser
 * does it.
 *
 * `url(\68ttps://…)` and `u\72l(…)` are the same fetch to a browser as the
 * plain spelling, so remote-reference detection has to look at what the CSS
 * *means*. A regex cannot do this soundly — `/*` inside a string is not a
 * comment, and inside an unquoted `url(` neither comments nor quotes are
 * special — so this walks the text once, tracking strings and url tokens.
 *
 * An escape that decodes to a quote, parenthesis or backslash is emitted as
 * `_`: escaped, those never end a string or a url in the browser, and keeping
 * them inert keeps this output's structure identical to the browser's view.
 * The result has no comments and no backslashes, so it reads the same to a
 * browser as it does to the checks below.
 */
export function normalizeCss(css) {
  const s = String(css ?? '');
  let out = '';
  let quote = null;      // inside a "…" or '…' string
  let inUrl = false;     // inside an unquoted url(…) token

  for (let i = 0; i < s.length; i++) {
    const c = s[i];

    if (c === '\\') {
      const hex = /^[0-9a-fA-F]{1,6}/.exec(s.slice(i + 1, i + 7));
      if (hex) {
        const cp = parseInt(hex[0], 16);
        const ch = cp > 0 && cp <= 0x10ffff && !(cp >= 0xd800 && cp <= 0xdfff)
          ? String.fromCodePoint(cp) : '�';
        i += hex[0].length;
        // One whitespace character after a hex escape belongs to the escape.
        if (s[i + 1] === '\r' && s[i + 2] === '\n') i += 2;
        else if (/[ \t\n\r\f]/.test(s[i + 1] ?? '')) i += 1;
        out += inert(ch);
      } else if (/[\n\r\f]/.test(s[i + 1] ?? '')) {
        i += s[i + 1] === '\r' && s[i + 2] === '\n' ? 2 : 1;   // line continuation
      } else if (i + 1 < s.length) {
        out += inert(s[i + 1]);
        i += 1;
      }
      continue;                                                // lone trailing "\" is dropped
    }

    if (inUrl) {
      if (c === ')') inUrl = false;
      out += c === '"' || c === "'" ? '_' : c;
      continue;
    }

    if (quote) {
      if (c === quote || c === '\n' || c === '\r' || c === '\f') quote = null;
      out += c;
      continue;
    }

    if (c === '"' || c === "'") {
      quote = c;
      out += c;
      continue;
    }

    if (c === '/' && s[i + 1] === '*') {
      const end = s.indexOf('*/', i + 2);
      i = end < 0 ? s.length : end + 1;
      continue;
    }

    if (c === '(' && /url$/i.test(out)) {
      const rest = s.slice(i + 1).replace(/^[ \t\n\r\f]+/, '');
      if (rest[0] !== '"' && rest[0] !== "'") inUrl = true;
    }
    out += c;
  }
  return out;
}

// A CSS string ends at an unescaped newline, so these must not span one either.
const CSS_URL = /url\(\s*(?:"([^"\n\r\f]*)"|'([^'\n\r\f]*)'|([^)"'\s]*))\s*\)/gi;
const CSS_STRING = /"([^"\n\r\f]*)"|'([^'\n\r\f]*)'/g;

/**
 * Remove every remote reference from a piece of CSS (a style attribute or the
 * text of a <style> element).
 *
 * `url(…)` tokens become `none`; any remaining *string* that is a remote URL
 * becomes `""`. The second rule is what covers `@import "…"`, `image-set("…")`
 * and `src("…")`, all of which fetch from a bare string — and the rare
 * false positive (a `content:` string that happens to start with `//`) costs
 * a little text, never a fetch.
 *
 * @returns {{ css: string, blocked: number }} `css` is the original text when
 *          nothing was found, so untouched styles are not rewritten at all.
 */
export function blockRemoteCss(css) {
  const original = String(css ?? '');
  const clean = normalizeCss(original);
  let blocked = 0;
  const out = clean
    .replace(CSS_URL, (whole, dq, sq, bare) => {
      if (!isRemoteUrl(dq ?? sq ?? bare ?? '')) return whole;
      blocked += 1;
      return 'none';
    })
    .replace(CSS_STRING, (whole, dq, sq) => {
      if (!isRemoteUrl(dq ?? sq ?? '')) return whole;
      blocked += 1;
      return '""';
    });
  // Unchanged meaning: keep the author's spelling. Changed: emit the normalised
  // form, because that is the only spelling that was actually checked.
  return blocked ? { css: out, blocked } : { css: original, blocked: 0 };
}

/**
 * True when CSS pulls a remote resource. CSS fetches just as effectively as
 * markup does.
 */
export function hasRemoteCssUrl(style) {
  return blockRemoteCss(style).blocked > 0;
}

/** Replace every remote reference in a piece of CSS. */
export function stripRemoteCssUrls(style) {
  return blockRemoteCss(style).css;
}

/**
 * Neutralise positioning that lets markup escape its container.
 *
 * Only matters where sanitised markup renders in the app's own document
 * (a quoted reply, a note): `position:fixed` there can lay a fake dialog or a
 * phishing link over the whole UI.
 */
export function stripEscapingPosition(style) {
  const original = String(style ?? '');
  const clean = normalizeCss(original);
  if (!/position\s*:\s*(fixed|sticky)/i.test(clean)) return original;
  return clean.replace(/position\s*:\s*(fixed|sticky)/gi, 'position:static');
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
