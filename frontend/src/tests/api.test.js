import { describe, it, expect, vi, beforeEach } from 'vitest';
import { parseAddresses, threadingHeaders } from '$lib/api.js';

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
// threadingHeaders
//
// Without In-Reply-To / References a reply starts a new conversation in every
// other mail client, which is the most visible way a webmail client feels
// half-finished.
// ---------------------------------------------------------------------------

describe('threadingHeaders', () => {
  it('returns nothing for a brand-new message', () => {
    expect(threadingHeaders(null, null)).toEqual({});
    expect(threadingHeaders(undefined, ['<a@x>'])).toEqual({});
  });

  it('sets In-Reply-To to the message being replied to', () => {
    expect(threadingHeaders('<parent@x>', null).inReplyTo).toEqual(['<parent@x>']);
  });

  it('appends the parent to the existing References chain', () => {
    const { references } = threadingHeaders('<parent@x>', ['<root@x>', '<mid@x>']);
    expect(references).toEqual(['<root@x>', '<mid@x>', '<parent@x>']);
  });

  it('starts a References chain when the parent had none', () => {
    expect(threadingHeaders('<parent@x>', null).references).toEqual(['<parent@x>']);
  });

  it('caps a long chain but always keeps the immediate parent last', () => {
    const long = Array.from({ length: 40 }, (_, i) => `<m${i}@x>`);
    const { references } = threadingHeaders('<parent@x>', long);
    expect(references).toHaveLength(20);
    expect(references.at(-1)).toBe('<parent@x>');
  });

  it('drops empty entries from the chain', () => {
    const { references } = threadingHeaders('<parent@x>', ['<root@x>', '', null]);
    expect(references).toEqual(['<root@x>', '<parent@x>']);
  });
});

// ---------------------------------------------------------------------------
// Calendar helpers
// ---------------------------------------------------------------------------

describe('getCalendarEvents', () => {
  beforeEach(() => {
    vi.stubGlobal('window', { location: { href: '' } });
    vi.stubGlobal('fetch', vi.fn());
  });

  it('returns empty array when server returns no events', async () => {
    fetch.mockResolvedValueOnce({
      status: 200,
      ok: true,
      json: async () => ({
        methodResponses: [
          ['CalendarEvent/query', { ids: [] }, 'q'],
          ['CalendarEvent/get',   { list: [] }, 'e'],
        ],
      }),
    });

    const { getCalendarEvents } = await import('$lib/api.js');
    const result = await getCalendarEvents('acc1', {}, null, null, null);
    expect(result).toEqual([]);
  });

  it('returns events from the second method response', async () => {
    const events = [{ id: 'ev1', title: 'Stand-up', start: '2025-05-01T09:00:00' }];
    fetch.mockResolvedValueOnce({
      status: 200,
      ok: true,
      json: async () => ({
        methodResponses: [
          ['CalendarEvent/query', { ids: ['ev1'] }, 'q'],
          ['CalendarEvent/get',   { list: events }, 'e'],
        ],
      }),
    });

    const { getCalendarEvents } = await import('$lib/api.js');
    const result = await getCalendarEvents('acc1', {}, null, null, null);
    expect(result).toEqual(events);
  });
});

// ---------------------------------------------------------------------------
// Contacts helpers
// ---------------------------------------------------------------------------

describe('getContacts', () => {
  beforeEach(() => {
    vi.stubGlobal('window', { location: { href: '' } });
    vi.stubGlobal('fetch', vi.fn());
  });

  it('returns empty array when server returns no contacts', async () => {
    fetch.mockResolvedValueOnce({
      status: 200,
      ok: true,
      json: async () => ({
        methodResponses: [
          ['ContactCard/query', { ids: [] }, 'q'],
          ['ContactCard/get',   { list: [] }, 'c'],
        ],
      }),
    });

    const { getContacts } = await import('$lib/api.js');
    const result = await getContacts('acc1', {}, null);
    expect(result).toEqual([]);
  });

  it('returns contacts from the second method response', async () => {
    const cards = [{ id: 'c1', fullName: 'Alice Smith', emails: { e1: { address: 'alice@example.com' } } }];
    fetch.mockResolvedValueOnce({
      status: 200,
      ok: true,
      json: async () => ({
        methodResponses: [
          ['ContactCard/query', { ids: ['c1'] }, 'q'],
          ['ContactCard/get',   { list: cards }, 'c'],
        ],
      }),
    });

    const { getContacts } = await import('$lib/api.js');
    const result = await getContacts('acc1', {}, null);
    expect(result).toEqual(cards);
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
