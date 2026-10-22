// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import { sanitizeEmailHtml } from '$lib/sanitize.js';
import { normalizeCss, blockRemoteCss, stripEscapingPosition, isRemoteUrl } from '$lib/urls.js';

// A <style> that opens a fragment is hoisted into <head> by the HTML parser
// and so never reaches a fragment render at all; body-position stylesheets are
// the ones that survive, so the fragment cases put content first.
//
// Every case here is a remote fetch that used to get past "remote images
// blocked". A miss is a read receipt the user never agreed to send.

const blocked = (html) => sanitizeEmailHtml(html);
const framed = (html) => sanitizeEmailHtml(html, { wholeDocument: true });

describe('remote content in <style> elements', () => {
  it('strips a remote url() from a stylesheet in the body', () => {
    const r = blocked('<p>hi</p><style>body{background:url(https://t.example/p.gif)}</style>');
    expect(r.html).not.toContain('t.example');
    expect(r.blocked).toBe(1);
  });

  it('strips a remote url() from a stylesheet in the head of a whole document', () => {
    const r = framed('<html><head><style>p{background:url(//t.example/p)}</style></head><body><p>x</p></body></html>');
    expect(r.html).not.toContain('t.example');
    expect(r.html).toContain('<style>');      // the stylesheet itself survives
    expect(r.blocked).toBe(1);
  });

  it('strips @import and image-set strings', () => {
    const r = blocked('<p>x</p><style>@import "https://t.example/a.css"; p{background:image-set("https://t.example/b.png" 1x)}</style>');
    expect(r.html).not.toContain('t.example');
    expect(r.blocked).toBe(2);
  });

  it('keeps local styles untouched', () => {
    const r = blocked('<p>x</p><style>p{color:red;background:url(data:image/png;base64,AAA)}</style>');
    expect(r.html).toContain('color:red');
    expect(r.blocked).toBe(0);
  });

  it('leaves stylesheets alone once the user allows images', () => {
    const r = sanitizeEmailHtml('<p>x</p><style>p{background:url(https://t.example/p)}</style>', { allowRemote: true });
    expect(r.html).toContain('t.example');
  });
});

describe('CSS spellings that mean the same fetch', () => {
  const remote = (css) => blockRemoteCss(css).blocked > 0;

  it('sees through hex escapes', () => {
    expect(remote('background:url(\\68ttps://t.example/p)')).toBe(true);
    expect(remote('background:u\\72l(https://t.example/p)')).toBe(true);
    expect(remote('background:url(/\\/t.example/p)')).toBe(true);
  });

  it('sees through comments between tokens', () => {
    expect(remote('background:/**/url(https://t.example/p)')).toBe(true);
  });

  it('does not treat /* inside a string as a comment', () => {
    expect(remote('content:"/*";background:url(//t.example/p);x:"*/"')).toBe(true);
  });

  it('does not treat /* inside an unquoted url as a comment', () => {
    expect(remote('background:url(//t.example/a/*);x:y /* */')).toBe(true);
  });

  it('keeps string boundaries where the browser keeps them', () => {
    expect(remote(`p{x:'\\'' "https://t.example/p" '}`)).toBe(true);
  });

  it('treats an http scheme without slashes as remote', () => {
    expect(remote('background:url(https:t.example/p)')).toBe(true);
  });

  it('emits CSS with no escapes or comments once something was removed', () => {
    const out = blockRemoteCss('color:red;/*c*/background:url(\\68ttps://t.example/p)').css;
    expect(out).not.toMatch(/\\|\/\*/);
    expect(out).toContain('color:red');
  });

  it('returns untouched CSS byte for byte', () => {
    const css = 'color: red; /* note */ font: 12px "A\\"B"';
    expect(blockRemoteCss(css)).toEqual({ css, blocked: 0 });
  });

  it('normalises escaped quotes to an inert character', () => {
    expect(normalizeCss('"a\\"b"')).toBe('"a_b"');
  });
});

describe('isRemoteUrl and the URL parser', () => {
  it('treats backslash forms of scheme-relative URLs as remote', () => {
    expect(isRemoteUrl('/\\t.example/p')).toBe(true);
    expect(isRemoteUrl('\\\\t.example/p')).toBe(true);
  });

  it('ignores tabs and newlines the parser would drop', () => {
    expect(isRemoteUrl('ht\ttps://t.example/p')).toBe(true);
    expect(isRemoteUrl('/\n/t.example/p')).toBe(true);
  });

  it('leaves same-origin paths alone', () => {
    expect(isRemoteUrl('/api/files/blob/x')).toBe(false);
    expect(isRemoteUrl('image.png')).toBe(false);
  });

  it('blocks those forms on img src', () => {
    const r = blocked('<img src="/\\t.example/p.gif">');
    expect(r.html).not.toMatch(/\ssrc=/);
    expect(r.blocked).toBe(1);
  });
});

describe('renders into the app document (inParent)', () => {
  const parent = (html) => sanitizeEmailHtml(html, { allowRemote: true, inParent: true }).html;

  it('removes <style> outright', () => {
    expect(parent('<p>x</p><style>body{display:none}</style>')).not.toContain('<style');
  });

  it('neutralises fixed and sticky positioning, including escaped spellings', () => {
    expect(parent('<div style="position:fixed;inset:0">x</div>')).not.toMatch(/fixed/);
    expect(parent('<div style="position: sticky">x</div>')).not.toMatch(/sticky/);
    expect(parent('<div style="posit\\69on:fixed">x</div>')).not.toMatch(/fixed/);
  });

  it('leaves ordinary positioning alone', () => {
    expect(stripEscapingPosition('position:relative')).toBe('position:relative');
  });

  it('keeps <style> in the sandboxed frame', () => {
    expect(sanitizeEmailHtml('<p>x</p><style>p{color:red}</style>').html).toContain('<style>');
  });
});

describe('form controls', () => {
  it('removes every kind, not only <form> and <input>', () => {
    const html = blocked('<select><option>a</option></select><textarea>t</textarea><dialog open>d</dialog>').html;
    expect(html).not.toMatch(/<(select|option|textarea|dialog)/);
  });
});
