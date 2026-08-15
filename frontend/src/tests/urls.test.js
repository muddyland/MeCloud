import { describe, it, expect } from 'vitest';
import {
  isRemoteUrl, hasRemoteCssUrl, stripRemoteCssUrls, escapeText, decodeEntities,
} from '$lib/urls.js';

// These back the remote-image blocking in the reading pane: anything
// isRemoteUrl() misses is a tracking pixel that loads without the user asking.

describe('isRemoteUrl', () => {
  it('treats absolute http(s) URLs as remote', () => {
    expect(isRemoteUrl('http://tracker.example/pixel.gif')).toBe(true);
    expect(isRemoteUrl('https://tracker.example/pixel.gif')).toBe(true);
  });

  it('treats protocol-relative URLs as remote', () => {
    expect(isRemoteUrl('//tracker.example/pixel.gif')).toBe(true);
  });

  it('ignores leading and trailing whitespace', () => {
    expect(isRemoteUrl('   https://tracker.example/p.gif  ')).toBe(true);
  });

  it('does not block inline or same-document references', () => {
    expect(isRemoteUrl('data:image/png;base64,iVBORw0KGgo=')).toBe(false);
    expect(isRemoteUrl('cid:part1.abc@example.com')).toBe(false);
    expect(isRemoteUrl('blob:https://app.example/1234')).toBe(false);
    expect(isRemoteUrl('#anchor')).toBe(false);
    expect(isRemoteUrl('about:blank')).toBe(false);
  });

  it('handles empty and non-string input', () => {
    expect(isRemoteUrl('')).toBe(false);
    expect(isRemoteUrl('   ')).toBe(false);
    expect(isRemoteUrl(null)).toBe(false);
    expect(isRemoteUrl(undefined)).toBe(false);
  });
});

describe('remote CSS urls', () => {
  it('detects a remote background image', () => {
    expect(hasRemoteCssUrl('background:url(https://tracker.example/bg.png)')).toBe(true);
    expect(hasRemoteCssUrl("background: url( 'https://t.example/b.png' )")).toBe(true);
    expect(hasRemoteCssUrl('background:url(//t.example/b.png)')).toBe(true);
  });

  it('leaves inline data urls alone', () => {
    expect(hasRemoteCssUrl('background:url(data:image/png;base64,AAA)')).toBe(false);
    expect(hasRemoteCssUrl('color: red')).toBe(false);
  });

  it('replaces remote urls but keeps the rest of the declaration', () => {
    const stripped = stripRemoteCssUrls('color:red;background:url(https://t.example/b.png)');
    expect(stripped).toContain('color:red');
    expect(stripped).not.toContain('t.example');
    expect(stripped).toContain('none');
  });
});

describe('escapeText', () => {
  it('escapes the characters that would start markup', () => {
    expect(escapeText('<script>alert(1)</script>'))
      .toBe('&lt;script&gt;alert(1)&lt;/script&gt;');
  });

  it('escapes ampersands before angle brackets so entities are not doubled', () => {
    expect(escapeText('a & b < c')).toBe('a &amp; b &lt; c');
  });

  it('handles nullish input', () => {
    expect(escapeText(null)).toBe('');
    expect(escapeText(undefined)).toBe('');
  });
});

describe('decodeEntities', () => {
  it('decodes the entities clients pre-encode into text/plain parts', () => {
    expect(decodeEntities('&gt; quoted &amp; escaped')).toBe('> quoted & escaped');
    expect(decodeEntities('&quot;hi&quot; &#39;there&#39;')).toBe('"hi" \'there\'');
  });

  it('round-trips safely through escapeText', () => {
    // This is the exact pipeline the plain-text path uses: decode what the
    // sender encoded, then re-escape so nothing renders as markup.
    const raw = '&lt;b&gt;not bold&lt;/b&gt;';
    expect(escapeText(decodeEntities(raw))).toBe('&lt;b&gt;not bold&lt;/b&gt;');
  });
});
