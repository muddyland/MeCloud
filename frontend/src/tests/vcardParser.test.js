import { describe, it, expect } from 'vitest';
import { parseVCard, decodeQuotedPrintable } from '$lib/vcardParser.js';

describe('decodeQuotedPrintable', () => {
  it('decodes UTF-8 byte sequences as one character', () => {
    expect(decodeQuotedPrintable('M=C3=BCller')).toBe('Müller');
  });

  it('honours a legacy CHARSET', () => {
    expect(decodeQuotedPrintable('M=FCller', 'ISO-8859-1')).toBe('Müller');
  });

  it('falls back to UTF-8 for an unknown charset', () => {
    expect(decodeQuotedPrintable('=C3=A9', 'X-NONSENSE')).toBe('é');
  });

  it('drops soft line breaks', () => {
    expect(decodeQuotedPrintable('ab=\r\ncd')).toBe('abcd');
  });
});

describe('parseVCard', () => {
  it('reads a vCard 3.0 card', () => {
    const [c] = parseVCard('BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Ada Lovelace\r\nEMAIL;TYPE=HOME:ada@x.test\r\nEND:VCARD\r\n');
    expect(c.name.full).toBe('Ada Lovelace');
    expect(Object.values(c.emails)[0].address).toBe('ada@x.test');
  });

  it('reads a vCard 2.1 quoted-printable name with a charset', () => {
    const [c] = parseVCard([
      'BEGIN:VCARD', 'VERSION:2.1',
      'FN;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:J=C3=BCrgen M=C3=BC',
      'END:VCARD',
    ].join('\r\n'));
    expect(c.name.full).toBe('Jürgen Mü');
  });

  it('joins quoted-printable values wrapped with a trailing =', () => {
    const [c] = parseVCard([
      'BEGIN:VCARD', 'VERSION:2.1',
      'FN;ENCODING=QUOTED-PRINTABLE:Fran=C3=A7oise =',
      'Dupont',
      'EMAIL:f@x.test',
      'END:VCARD',
    ].join('\r\n'));
    expect(c.name.full).toBe('Françoise Dupont');
    expect(Object.values(c.emails)[0].address).toBe('f@x.test');
  });

  it('reads several cards and skips nameless ones', () => {
    const cards = parseVCard('BEGIN:VCARD\nFN:A\nEND:VCARD\nBEGIN:VCARD\nEMAIL:x@y\nEND:VCARD\nBEGIN:VCARD\nN:Doe;Jane\nEND:VCARD\n');
    expect(cards.map((c) => c.name.full)).toEqual(['A', 'Jane Doe']);
  });
});
