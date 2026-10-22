import { describe, it, expect } from 'vitest';
import { toCardPatch, safeWebsiteHref } from '$lib/contactCard.js';

describe('toCardPatch', () => {
  it('nulls every edited property the form left empty', () => {
    const patch = toCardPatch({ '@type': 'Card', name: { full: 'A' } });
    for (const key of ['organizations', 'titles', 'emails', 'phones', 'addresses', 'links', 'notes', 'anniversaries']) {
      expect(patch[key]).toBeNull();
    }
    expect(patch.name.full).toBe('A');
  });

  it('keeps what the form filled in', () => {
    const links = { w1: { '@type': 'Link', href: 'https://a.example' } };
    expect(toCardPatch({ links }).links).toBe(links);
  });

  it('keeps anniversaries the form does not edit', () => {
    const existing = { anniversaries: {
      birth: { kind: 'birth', date: { year: 1990 } },
      wed: { kind: 'wedding', date: { year: 2020 } },
    } };
    const patch = toCardPatch({}, existing);
    expect(patch.anniversaries).toEqual({ wed: existing.anniversaries.wed });
  });
});

describe('safeWebsiteHref', () => {
  it('links http and https', () => {
    expect(safeWebsiteHref('https://a.example/x')).toBe('https://a.example/x');
    expect(safeWebsiteHref(' http://a.example ')).toBe('http://a.example');
  });

  it('adds https to a bare host', () => {
    expect(safeWebsiteHref('a.example/path')).toBe('https://a.example/path');
  });

  it('refuses every other scheme', () => {
    expect(safeWebsiteHref('javascript:alert(1)')).toBeNull();
    expect(safeWebsiteHref('data:text/html,x')).toBeNull();
    expect(safeWebsiteHref('JaVaScRiPt:alert(1)')).toBeNull();
    expect(safeWebsiteHref('')).toBeNull();
  });
});
