import { describe, it, expect } from 'vitest';
import { readPersisted } from '$lib/stores/persisted.js';

describe('readPersisted', () => {
  it('uses the default when nothing is stored', () => {
    expect(readPersisted(null, 240)).toBe(240);
  });

  it('survives a corrupt entry instead of throwing at boot', () => {
    expect(readPersisted('{not json', 240)).toBe(240);
    expect(readPersisted('undefined', 'grid')).toBe('grid');
  });

  it('rejects a value of the wrong type', () => {
    expect(readPersisted('"wide"', 240)).toBe(240);
    expect(readPersisted('null', 240)).toBe(240);
  });

  it('clamps numbers', () => {
    expect(readPersisted('9999', 240, { min: 180, max: 380 })).toBe(380);
    expect(readPersisted('10', 240, { min: 180, max: 380 })).toBe(180);
  });

  it('enforces an allow-list', () => {
    expect(readPersisted('"list"', 'grid', { allowed: ['grid', 'list'] })).toBe('list');
    expect(readPersisted('"table"', 'grid', { allowed: ['grid', 'list'] })).toBe('grid');
  });
});
