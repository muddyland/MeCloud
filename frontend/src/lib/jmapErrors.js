/**
 * JMAP error handling, per RFC 8620.
 *
 * The spec draws a distinction this app previously ignored entirely:
 *
 *  • §3.6.2 *method-level* errors replace the whole response for one call —
 *    `["error", {"type": "..."}, "callId"]` sits in `methodResponses` where the
 *    normal arguments would be. Nothing here used to look for those, so such a
 *    response read as "no data" and failed silently.
 *
 *  • §5.3 *SetError* objects live inside a successful `/set` response and
 *    describe individual records that could not be created, updated or
 *    destroyed.
 *
 * They have separate, non-overlapping type vocabularies. Treating them as one
 * list is why `stateMismatch` and `accountReadOnly` were previously listed as
 * SetError types — they are method-level errors.
 */

/**
 * SetError types, RFC 8620 §5.3.
 *
 * The spec is explicit that a SetError's `description` is "a non-localised
 * string and is not intended to be shown directly to end users" — it exists for
 * debugging. So the *type* is what gets turned into user-facing text here, and
 * the description is only used when the type is one we do not recognise.
 */
const SET_ERROR_MESSAGES = {
  forbidden:         'The server refused that — you may not have permission.',
  overQuota:         'There is not enough storage space left.',
  tooLarge:          'That is larger than the server will accept.',
  rateLimit:         'The server is rate limiting requests — try again shortly.',
  notFound:          'That item no longer exists on the server.',
  invalidPatch:      'That change could not be applied.',
  willDestroy:       'That item is already being deleted.',
  invalidProperties: 'The server rejected one of the values.',
  singleton:         'That item cannot be created or deleted.',

  // Defined by §5.4 for /copy, not /set. Stalwart returns it from FileNode/set
  // when a name is taken, and supplies the `existingId` that /copy requires —
  // a sensible extension, handled here because servers do use it this way.
  alreadyExists:     'Something with that name already exists here.',
};

/** Method-level error types, RFC 8620 §3.6.2, plus the per-method additions. */
const METHOD_ERROR_MESSAGES = {
  serverUnavailable:          'The server is temporarily unavailable — try again shortly.',
  serverFail:                 'The server hit an internal error handling that request.',
  serverPartialFail:          'The server only partly completed that request.',
  unknownMethod:              'This server does not support that operation.',
  invalidArguments:           'The server rejected the request as malformed.',
  invalidResultReference:     'The server could not resolve part of the request.',
  forbidden:                  'The server refused that — you may not have permission.',
  accountNotFound:            'That account no longer exists on the server.',
  accountNotSupportedByMethod:'This account does not support that operation.',
  accountReadOnly:            'This account is read-only.',
  // Returned by /set and /changes respectively.
  stateMismatch:              'Something else changed this first — reload and try again.',
  cannotCalculateChanges:     'The server could not work out what changed — reload.',
};

function describe(error, table, fallback) {
  if (!error) return fallback;

  const type = typeof error.type === 'string' ? error.type : '';
  const mapped = table[type];

  if (mapped) {
    const props = Array.isArray(error.properties) ? error.properties.filter(Boolean) : [];
    return props.length ? `${mapped} (${props.join(', ')})` : mapped;
  }

  // Unrecognised type: the description is a debug string, but it beats nothing,
  // and the raw type is worth keeping so it can be quoted in a bug report.
  const description = typeof error.description === 'string' ? error.description.trim() : '';
  if (type && description) return `${fallback} (${type}: ${description})`;
  if (type) return `${fallback} (${type})`;
  if (description) return `${fallback} (${description})`;
  return fallback;
}

export function setErrorMessage(error, fallback = 'The server rejected that request.') {
  return describe(error, SET_ERROR_MESSAGES, fallback);
}

export function methodErrorMessage(error, fallback = 'The server rejected that request.') {
  return describe(error, METHOD_ERROR_MESSAGES, fallback);
}

/** An error describing a single record a /set refused. */
export class JmapSetError extends Error {
  constructor(error, fallback) {
    super(setErrorMessage(error, fallback));
    this.name = 'JmapSetError';
    this.type = error?.type ?? null;
    // §5.4 requires this on alreadyExists, and Stalwart supplies it.
    this.existingId = error?.existingId ?? null;
    this.properties = Array.isArray(error?.properties) ? error.properties : null;
  }
}

/** An error returned in place of a whole method response. */
export class JmapMethodError extends Error {
  constructor(error, callId) {
    super(methodErrorMessage(error));
    this.name = 'JmapMethodError';
    this.type = error?.type ?? null;
    this.callId = callId ?? null;
  }
}

/**
 * Find a method-level error in a response.
 *
 * §3.6.2 puts these in `methodResponses` under the name "error", exactly where
 * a caller expects its arguments. Reading `methodResponses[0][1]` and looking
 * for `list` or `created` therefore finds nothing and reports no failure, which
 * is the quietest possible way for a request to go wrong.
 *
 * @returns {JmapMethodError|null}
 */
export function findMethodError(data) {
  for (const entry of data?.methodResponses ?? []) {
    if (Array.isArray(entry) && entry[0] === 'error') {
      return new JmapMethodError(entry[1], entry[2]);
    }
  }
  return null;
}
