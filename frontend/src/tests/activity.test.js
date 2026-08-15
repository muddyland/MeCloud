import { describe, it, expect } from 'vitest';
import { get } from 'svelte/store';
import { busy, inFlight, begin, end, tracked } from '$lib/stores/activity.js';

// This counter drives the progress bar under the navbar. If it ever fails to
// return to zero the bar sticks on forever, so the balancing is what matters.

describe('activity store', () => {
  it('starts idle', () => {
    expect(get(busy)).toBe(false);
    expect(get(inFlight)).toBe(0);
  });

  it('is busy while a request is outstanding', () => {
    begin();
    expect(get(busy)).toBe(true);
    end();
    expect(get(busy)).toBe(false);
  });

  it('stays busy until the last of several requests finishes', () => {
    begin(); begin(); begin();
    expect(get(inFlight)).toBe(3);
    end(); end();
    expect(get(busy)).toBe(true);
    end();
    expect(get(busy)).toBe(false);
  });

  it('never goes negative on an unbalanced end()', () => {
    end(); end();
    expect(get(inFlight)).toBe(0);
    begin();
    expect(get(busy)).toBe(true);
    end();
    expect(get(busy)).toBe(false);
  });

  it('tracked() returns the wrapped value and clears the counter', async () => {
    const result = await tracked(async () => 'done');
    expect(result).toBe('done');
    expect(get(inFlight)).toBe(0);
  });

  it('tracked() clears the counter when the work throws', async () => {
    await expect(tracked(async () => { throw new Error('boom'); })).rejects.toThrow('boom');
    expect(get(inFlight)).toBe(0);
  });
});
