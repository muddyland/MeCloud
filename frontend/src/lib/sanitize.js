import DOMPurify from 'dompurify';
import { isRemoteUrl, hasRemoteCssUrl, stripRemoteCssUrls } from './urls.js';

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
  FORBID_TAGS: ['script', 'object', 'embed', 'form', 'input', 'button', 'iframe', 'meta', 'link'],
  FORBID_ATTR: ['formaction', 'ping', 'srcdoc'],
  ALLOW_DATA_ATTR: false,
  ALLOW_UNKNOWN_PROTOCOLS: false,
};

// Attributes that can pull down a remote resource just by existing.
const RESOURCE_ATTRS = ['src', 'srcset', 'poster', 'background', 'lowsrc'];

let blockRemote = true;
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

    if (!blockRemote) return;

    for (const attr of RESOURCE_ATTRS) {
      const value = node.getAttribute(attr);
      if (value && isRemoteUrl(value)) {
        node.removeAttribute(attr);
        node.setAttribute(`data-blocked-${attr}`, value);
        blockedCount += 1;
      }
    }

    const style = node.getAttribute('style');
    if (style && hasRemoteCssUrl(style)) {
      node.setAttribute('style', stripRemoteCssUrls(style));
      blockedCount += 1;
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
 * @returns {{ html: string, blocked: number }} sanitised markup and the number
 *          of remote resources withheld
 */
export function sanitizeEmailHtml(html, { allowRemote = false, wholeDocument = false } = {}) {
  installHooks();
  blockRemote = !allowRemote;
  blockedCount = 0;
  try {
    const clean = DOMPurify.sanitize(html ?? '', {
      ...PURIFY_OPTS,
      WHOLE_DOCUMENT: wholeDocument,
    });
    return { html: clean, blocked: blockedCount };
  } finally {
    blockRemote = true;
    blockedCount = 0;
  }
}
