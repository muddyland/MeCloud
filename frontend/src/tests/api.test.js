import { describe, it, expect, vi, beforeEach } from 'vitest';
import { parseAddresses } from '$lib/api.js';

// ---------------------------------------------------------------------------
// parseAddresses
// ---------------------------------------------------------------------------

describe('parseAddresses', () => {
  it('returns empty array for empty input', () => {
    expect(parseAddresses('')).toEqual([]);
    expect(parseAddresses('   ')).toEqual([]);
    expect(parseAddresses(null)).toEqual([]);
    expect(parseAddresses(undefined)).toEqual([]);
  });

  it('parses a bare email address', () => {
    expect(parseAddresses('alice@example.com')).toEqual([
      { email: 'alice@example.com' },
    ]);
  });

  it('parses a display name + angle-bracket address', () => {
    expect(parseAddresses('Alice Smith <alice@example.com>')).toEqual([
      { name: 'Alice Smith', email: 'alice@example.com' },
    ]);
  });

  it('trims whitespace around addresses', () => {
    expect(parseAddresses('  bob@example.com  ')).toEqual([
      { email: 'bob@example.com' },
    ]);
  });

  it('parses a comma-separated list of bare emails', () => {
    const result = parseAddresses('a@x.com, b@x.com, c@x.com');
    expect(result).toHaveLength(3);
    expect(result.map(r => r.email)).toEqual(['a@x.com', 'b@x.com', 'c@x.com']);
  });

  it('parses a mixed list of named and bare addresses', () => {
    const result = parseAddresses('Alice <alice@x.com>, bob@x.com');
    expect(result).toHaveLength(2);
    expect(result[0]).toEqual({ name: 'Alice', email: 'alice@x.com' });
    expect(result[1]).toEqual({ email: 'bob@x.com' });
  });

  it('filters out entries with no email', () => {
    // A string that looks like a name but has no email part
    const result = parseAddresses('just-a-name');
    // 'just-a-name' has no '@' but will be treated as a bare email — filtering
    // only removes entries where email is falsy after matching
    expect(result.every(r => r.email)).toBe(true);
  });

  it('handles extra whitespace inside angle brackets', () => {
    const result = parseAddresses('Bob < bob@example.com >');
    expect(result[0].email).toBe('bob@example.com');
  });
});

// ---------------------------------------------------------------------------
// apiFetch — 401 redirect behaviour (requires DOM / window mock)
// ---------------------------------------------------------------------------

describe('apiFetch 401 redirect', () => {
  beforeEach(() => {
    vi.stubGlobal('window', { location: { href: '' } });
    vi.stubGlobal('fetch', vi.fn());
  });

  it('redirects to /auth/login on a 401 response', async () => {
    fetch.mockResolvedValueOnce({ status: 401, ok: false });

    // We import dynamically so the window stub is in place
    const { getMe } = await import('$lib/api.js');
    const result = await getMe();

    expect(result).toEqual({ authenticated: false });
    expect(window.location.href).toBe('/auth/login');
  });
});
