import { describe, it, expect } from 'vitest';
import {
  escapeSieveString, escapeMatchLiteral, commentSafe, buildCondition,
  generateSieve, extractRules,
} from '$lib/sieve.js';

const rule = (over = {}) => ({
  id: 'r1', name: 'Rule', enabled: true, match: 'all',
  conditions: [{ field: 'subject', op: 'contains', value: 'x' }],
  actions: [{ type: 'discard' }],
  ...over,
});

describe('quoting', () => {
  it('escapes backslash and quote in strings', () => {
    expect(escapeSieveString('a"b\\c')).toBe('a\\"b\\\\c');
  });

  it('cannot be broken out of with a quote', () => {
    const test = buildCondition({ field: 'subject', op: 'is', value: '" { discard; } if true "' });
    expect(test).toBe('header :is "Subject" "\\" { discard; } if true \\""');
  });

  it('treats * and ? literally in starts/ends with', () => {
    expect(escapeMatchLiteral('a*b?')).toBe('a\\*b\\?');
    expect(buildCondition({ field: 'subject', op: 'starts_with', value: 'a*b' }))
      .toBe('header :matches "Subject" "a\\\\*b*"');
  });

  it('tolerates non-string values from older stored rules', () => {
    expect(buildCondition({ field: 'subject', op: 'is', value: 42 })).toBe('header :is "Subject" "42"');
    expect(escapeSieveString(null)).toBe('');
  });

  it('refuses header names it does not offer', () => {
    expect(buildCondition({ field: 'X-Evil" :all', op: 'is', value: 'v' })).toBeNull();
  });
});

describe('generateSieve', () => {
  it('keeps a rule name on its comment line', () => {
    expect(commentSafe('a\nif true { discard; }')).toBe('a if true { discard; }');
    const script = generateSieve([rule({ name: 'x\nkeep;' })]);
    expect(script).toContain('# Rule: x keep;');
    expect(script.split('\n').filter((l) => l.trim() === 'keep;')).toHaveLength(0);
  });

  it('skips incomplete rules but stores them for the editor', () => {
    const half = rule({ conditions: [{ field: 'from', op: 'is', value: '' }] });
    const script = generateSieve([half]);
    expect(script).not.toContain('if ');
    expect(extractRules(script).rules).toEqual([half]);
  });

  it('requires imap4flags only when flags are used', () => {
    expect(generateSieve([rule()])).toContain('require ["fileinto"];');
    expect(generateSieve([rule({ actions: [{ type: 'mark_read' }] })]))
      .toContain('require ["fileinto", "imap4flags"];');
  });

  it('round-trips unicode rule state', () => {
    const r = rule({ name: 'Rechnungen €', conditions: [{ field: 'from', op: 'contains', value: 'bücher' }] });
    expect(extractRules(generateSieve([r])).rules[0].name).toBe('Rechnungen €');
  });
});

describe('extractRules', () => {
  it('flags a script this editor did not write', () => {
    expect(extractRules('require "fileinto";\nkeep;')).toEqual({ rules: [], custom: true });
    expect(extractRules('')).toEqual({ rules: [], custom: false });
  });
});
