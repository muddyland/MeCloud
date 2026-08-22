import { describe, it, expect } from 'vitest';
import {
  setErrorMessage, methodErrorMessage, findMethodError,
  JmapSetError, JmapMethodError,
} from '$lib/jmapErrors.js';

/*
 * Checked against RFC 8620 rather than memory. Two things that check corrected:
 *
 *  • §5.3 says a SetError's `description` is "a non-localised string and is not
 *    intended to be shown directly to end users" — it is for debugging. The
 *    user-facing text therefore comes from `type`, not `description`.
 *  • §3.6.2 method-level errors are a separate vocabulary from SetError.
 *    `stateMismatch`, `invalidArguments` and `accountReadOnly` are method-level,
 *    not SetError types, and had been miscategorised here.
 */

describe('setErrorMessage', () => {
  it('maps the type rather than showing the debug description', () => {
    // description is explicitly not for end users, so the mapped type wins.
    const msg = setErrorMessage({ type: 'overQuota', description: 'mbox quota 1048576 exceeded' });
    expect(msg).toMatch(/storage space/i);
    expect(msg).not.toContain('1048576');
  });

  it('handles the exact payload this server returned', () => {
    expect(setErrorMessage({ type: 'alreadyExists', existingId: '1x' }))
      .toMatch(/already exists/i);
  });

  it('covers every SetError type RFC 8620 §5.3 defines', () => {
    for (const type of ['forbidden', 'overQuota', 'tooLarge', 'rateLimit', 'notFound',
                        'invalidPatch', 'willDestroy', 'invalidProperties', 'singleton']) {
      expect(setErrorMessage({ type }), `unmapped SetError type: ${type}`)
        .not.toMatch(/rejected that request/);
    }
  });

  it('names the offending fields for invalidProperties', () => {
    expect(setErrorMessage({ type: 'invalidProperties', properties: ['name', 'parentId'] }))
      .toContain('name, parentId');
  });

  it('keeps an unknown type, and its description, rather than dropping them', () => {
    expect(setErrorMessage({ type: 'wibble' }, 'Create failed')).toBe('Create failed (wibble)');
    expect(setErrorMessage({ type: 'wibble', description: 'why' }, 'Create failed'))
      .toBe('Create failed (wibble: why)');
  });

  it('falls back cleanly with nothing to go on', () => {
    expect(setErrorMessage(null, 'Create failed')).toBe('Create failed');
    expect(setErrorMessage({}, 'Create failed')).toBe('Create failed');
  });
});

describe('methodErrorMessage', () => {
  it('covers every method-level type RFC 8620 §3.6.2 defines', () => {
    for (const type of ['serverUnavailable', 'serverFail', 'serverPartialFail',
                        'unknownMethod', 'invalidArguments', 'invalidResultReference',
                        'forbidden', 'accountNotFound', 'accountNotSupportedByMethod',
                        'accountReadOnly']) {
      expect(methodErrorMessage({ type }), `unmapped method error: ${type}`)
        .not.toMatch(/rejected that request/);
    }
  });

  it('covers the per-method additions', () => {
    expect(methodErrorMessage({ type: 'stateMismatch' })).toMatch(/reload/i);
    expect(methodErrorMessage({ type: 'cannotCalculateChanges' })).toMatch(/reload/i);
  });
});

describe('findMethodError', () => {
  it('finds an error response sitting where arguments should be', () => {
    // §3.6.2: the error replaces the whole response for that call.
    const err = findMethodError({
      methodResponses: [['error', { type: 'accountReadOnly' }, 'f']],
    });
    expect(err).toBeInstanceOf(JmapMethodError);
    expect(err.type).toBe('accountReadOnly');
    expect(err.callId).toBe('f');
    expect(err.message).toMatch(/read-only/i);
  });

  it('finds an error among otherwise successful calls', () => {
    const err = findMethodError({
      methodResponses: [
        ['Mailbox/get', { list: [] }, 'mb'],
        ['error', { type: 'unknownMethod' }, 'fn'],
      ],
    });
    expect(err?.type).toBe('unknownMethod');
  });

  it('returns null for a clean response', () => {
    expect(findMethodError({ methodResponses: [['Mailbox/get', { list: [] }, 'mb']] })).toBeNull();
    expect(findMethodError({ methodResponses: [] })).toBeNull();
    expect(findMethodError({})).toBeNull();
    expect(findMethodError(null)).toBeNull();
  });

  it('does not mistake a normal response whose name merely contains "error"', () => {
    expect(findMethodError({ methodResponses: [['Foo/errorLog', { list: [] }, 'x'] ] })).toBeNull();
  });
});

describe('JmapSetError', () => {
  it('keeps the machine-readable parts so callers can branch', () => {
    const err = new JmapSetError({ type: 'alreadyExists', existingId: '1x' }, 'Create failed');
    expect(err).toBeInstanceOf(Error);
    expect(err.type).toBe('alreadyExists');
    // §5.4 requires existingId on alreadyExists; it is what lets the UI show
    // the item that is already there instead of only complaining.
    expect(err.existingId).toBe('1x');
  });

  it('degrades safely for a malformed error', () => {
    const err = new JmapSetError(undefined, 'Delete failed');
    expect(err.message).toBe('Delete failed');
    expect(err.type).toBeNull();
  });
});
