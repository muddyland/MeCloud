import DOMPurify from 'dompurify';
import { isRemoteUrl, blockRemoteCss, stripEscapingPosition } from './urls.js';

export { escapeText, decodeEntities, isRemoteUrl } from './urls.js';

/**
 * Email HTML sanitisation, plus remote-content blocking.
 *
 * Two independent protections are at work here and it is worth being explicit
 * about which does what:
 *
 *  1. Script execution is prevented by the *iframe*, which is rendered with a
 *     `sandbox` attribute that omits `allow-scripts` and `allow-same-origin`.
 *     Nothing in a message can run code or reach our origin, whatever this
 *     module does.
 *  2. This module strips dangerous markup as defence in depth, and — the part
 *     that actually changes behaviour users can see — withholds remote images
 *     until asked. A remote `<img>` in an email is a tracking pixel: loading it
 *     tells the sender the message was opened, when, and from which IP. Every
 *     serious mail client blocks these by default; this one now does too.
 */

const PURIFY_OPTS = {
  USE_PROFILES: { html: true },
  // Form controls go too, not only <form>: nothing in a message or a note has
  // a legitimate reason to put an input in front of the user.
  FORBID_TAGS: [
    'script', 'object', 'embed', 'form', 'input', 'button', 'iframe', 'meta', 'link',
    'select', 'option', 'optgroup', 'textarea', 'datalist', 'dialog',
  ],
  FORBID_ATTR: ['formaction', 'ping', 'srcdoc'],
  ALLOW_DATA_ATTR: false,
  ALLOW_UNKNOWN_PROTOCOLS: false,
};

// Attributes that can pull down a remote resource just by existing.
const RESOURCE_ATTRS = ['src', 'srcset', 'poster', 'background', 'lowsrc'];

let blockRemote = true;
let inParent = false;
let blockedCount = 0;
let hooksInstalled = false;

/**
 * Hooks are installed on first use rather than at import time: DOMPurify has
 * no hook machinery when it loads without a DOM, and this module gets imported
 * by tooling that runs outside the browser.
 */
function installHooks() {
  if (hooksInstalled || typeof DOMPurify.addHook !== 'function') return;
  hooksInstalled = true;

  DOMPurify.addHook('afterSanitizeAttributes', (node) => {
    if (typeof node.getAttribute !== 'function') return;

    // Any surviving link must open outside the sandbox and must not hand the
    // destination our URL in the Referer header.
    if (node.tagName === 'A' && node.hasAttribute('href')) {
      node.setAttribute('target', '_blank');
      node.setAttribute('rel', 'noopener noreferrer nofollow');
    }

    if (blockRemote) {
      for (const attr of RESOURCE_ATTRS) {
        const value = node.getAttribute(attr);
        if (value && isRemoteUrl(value)) {
          node.removeAttribute(attr);
          node.setAttribute(`data-blocked-${attr}`, value);
          blockedCount += 1;
        }
      }
    }

    let style = node.getAttribute('style');
    if (style && inParent) {
      // Markup rendered into the app's own document must stay inside its box.
      const contained = stripEscapingPosition(style);
      if (contained !== style) node.setAttribute('style', (style = contained));
    }

    if (!blockRemote || !style) return;
    const css = blockRemoteCss(style);
    if (css.blocked) {
      node.setAttribute('style', css.css);
      blockedCount += css.blocked;
    }
  });

  // <style> elements fetch exactly like style attributes do — a background on
  // `body` in a stylesheet is as good a tracking pixel as an <img> — so their
  // text gets the same treatment. Only reached for the sandboxed frame: renders
  // into the parent document forbid <style> outright (see sanitizeEmailHtml).
  DOMPurify.addHook('uponSanitizeElement', (node, data) => {
    if (data.tagName !== 'style' || !blockRemote) return;
    const css = blockRemoteCss(node.textContent);
    if (css.blocked) {
      node.textContent = css.css;
      blockedCount += css.blocked;
    }
  });
}

/**
 * Sanitise message HTML.
 *
 * @param {string} html            raw HTML from the message body
 * @param {object} [options]
 * @param {boolean} [options.allowRemote=false]   load remote images
 * @param {boolean} [options.wholeDocument=false] keep <html>/<head> structure
 * @param {boolean} [options.inParent=false]      the result goes into the app's
 *        own document (a quoted reply, a note) rather than the sandboxed frame:
 *        <style> is removed, since a stylesheet there restyles the whole app,
 *        and fixed/sticky positioning is neutralised
 * @returns {{ html: string, blocked: number }} sanitised markup and the number
 *          of remote resources withheld
 */
export function sanitizeEmailHtml(html, { allowRemote = false, wholeDocument = false, inParent: parent = false } = {}) {
  installHooks();
  blockRemote = !allowRemote;
  inParent = parent;
  blockedCount = 0;
  try {
    const clean = DOMPurify.sanitize(html ?? '', {
      ...PURIFY_OPTS,
      FORBID_TAGS: parent ? [...PURIFY_OPTS.FORBID_TAGS, 'style'] : PURIFY_OPTS.FORBID_TAGS,
      WHOLE_DOCUMENT: wholeDocument,
    });
    return { html: clean, blocked: blockedCount };
  } finally {
    blockRemote = true;
    inParent = false;
    blockedCount = 0;
  }
}
