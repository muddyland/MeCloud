import { describe, it, expect } from 'vitest';
import { interpretCreate } from '$lib/files.js';

/*
 * Regression cover for folder creation breaking outright.
 *
 * A JMAP `/set` response reports refusals in `notCreated`. It is *not* required
 * to echo the new object back in `created`. An earlier version treated a
 * missing `created` entry as a failure, which rejected writes the server had
 * actually performed and stopped folder creation working at all.
 */

describe('interpretCreate', () => {
  it('returns the node when the server echoes it', () => {
    const { node, error } = interpretCreate({ created: { nf: { id: 'F1', size: 0 } } }, 'nf');
    expect(node).toEqual({ id: 'F1', size: 0 });
    expect(error).toBeNull();
  });

  it('reports an explicit refusal', () => {
    const refusal = { type: 'forbidden', description: 'Nope' };
    const { node, error } = interpretCreate({ notCreated: { nf: refusal } }, 'nf');
    expect(node).toBeNull();
    expect(error).toEqual(refusal);
  });

  it('treats a missing created entry as unconfirmed, NOT as an error', () => {
    // The exact shape that broke folder creation.
    for (const resp of [{}, { created: {} }, { created: null }, undefined]) {
      const { node, error } = interpretCreate(resp, 'nf');
      expect(error, `should not be an error for ${JSON.stringify(resp)}`).toBeNull();
      expect(node).toBeNull();
    }
  });

  it('treats an echoed object without an id as unconfirmed', () => {
    // Nothing can be done with an id-less node, but it is still not a refusal.
    const { node, error } = interpretCreate({ created: { nf: { size: 0 } } }, 'nf');
    expect(node).toBeNull();
    expect(error).toBeNull();
  });

  it('only looks at its own creation id', () => {
    const resp = { created: { other: { id: 'X' } }, notCreated: { other: { description: 'no' } } };
    expect(interpretCreate(resp, 'nf')).toEqual({ node: null, error: null });
  });

  it('prefers the refusal when a response somehow carries both', () => {
    const resp = { created: { nf: { id: 'F1' } }, notCreated: { nf: { description: 'no' } } };
    expect(interpretCreate(resp, 'nf').error).not.toBeNull();
  });
});
