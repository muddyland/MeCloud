import { describe, it, expect } from 'vitest';
import { parseRetryAfter, retryDelayMs } from '$lib/files.js';

// Dropping a folder fires one upload per file, so hitting the per-minute limit
// is a normal consequence of normal use. These govern waiting it out rather
// than failing the upload.

describe('parseRetryAfter', () => {
  it('reads delta-seconds', () => {
    expect(parseRetryAfter('5')).toBe(5000);
    expect(parseRetryAfter('0')).toBe(0);
    expect(parseRetryAfter(' 12 ')).toBe(12000);
  });

  it('reads an HTTP date, relative to now', () => {
    const now = Date.parse('2026-01-01T00:00:00Z');
    expect(parseRetryAfter('Thu, 01 Jan 2026 00:00:30 GMT', now)).toBe(30000);
  });

  it('never returns a negative wait for a date in the past', () => {
    const now = Date.parse('2026-01-01T00:01:00Z');
    expect(parseRetryAfter('Thu, 01 Jan 2026 00:00:00 GMT', now)).toBe(0);
  });

  it('returns null when absent or unparseable', () => {
    expect(parseRetryAfter(null)).toBeNull();
    expect(parseRetryAfter(undefined)).toBeNull();
    expect(parseRetryAfter('')).toBeNull();
    expect(parseRetryAfter('soon')).toBeNull();
  });
});

describe('retryDelayMs', () => {
  const fixed = { random: () => 0 };

  it('honours Retry-After over its own backoff', () => {
    expect(retryDelayMs(0, '7', fixed)).toBe(7000);
    // Even on a late attempt, the server's advice wins.
    expect(retryDelayMs(3, '2', fixed)).toBe(2000);
  });

  it('backs off exponentially without a header', () => {
    expect(retryDelayMs(0, null, fixed)).toBe(1000);
    expect(retryDelayMs(1, null, fixed)).toBe(2000);
    expect(retryDelayMs(2, null, fixed)).toBe(4000);
    expect(retryDelayMs(3, null, fixed)).toBe(8000);
  });

  it('adds jitter so a batch does not resume in lockstep', () => {
    expect(retryDelayMs(0, null, { random: () => 1 })).toBe(1500);
    expect(retryDelayMs(0, null, { random: () => 0 })).toBe(1000);
  });

  it('caps the wait, however large the advice or the attempt', () => {
    expect(retryDelayMs(0, '99999', fixed)).toBe(30_000);
    expect(retryDelayMs(20, null, fixed)).toBe(30_000);
  });
});
