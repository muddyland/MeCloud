import { begin, end } from '$lib/stores/activity.js';
import { JmapSetError, findMethodError } from './jmapErrors.js';

// A request that never settles leaves a spinner turning forever. Everything
// except the event stream is bounded.
const REQUEST_TIMEOUT_MS = 30_000;

// One redirect to the login page per page load. Without this guard a backend
// that answers 401 to everything turns concurrent requests into a redirect
// storm, and the user sees the login page flicker instead of load.
let redirectingToLogin = false;

async function apiFetch(url, options = {}) {
  const controller = typeof AbortController !== 'undefined' ? new AbortController() : null;
  const timer = controller
    ? setTimeout(() => controller.abort(new Error('Request timed out')), REQUEST_TIMEOUT_MS)
    : null;

  begin();
  let res;
  try {
    res = await fetch(url, {
      credentials: 'include',
      signal: controller?.signal,
      ...options,
    });
  } finally {
    if (timer) clearTimeout(timer);
    end();
  }

  if (res.status === 401 && typeof window !== 'undefined') {
    // Session expired — send the user back to the login flow
    if (!redirectingToLogin) {
      redirectingToLogin = true;
      window.location.href = '/auth/login';
    }
    return null;
  }
  return res;
}

/**
 * Issue a JMAP request through the backend proxy.
 *
 * Exported as `jmapPost` so sibling modules (files.js) reuse this one
 * transport — with its timeout, 401 handling, activity tracking and error
 * message extraction — rather than reimplementing fetch against /api/jmap.
 */
export async function jmapPost(methodCalls, using = ['urn:ietf:params:jmap:core', 'urn:ietf:params:jmap:mail']) {
  const res = await apiFetch('/api/jmap', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ using, methodCalls }),
  });
  if (!res) return null;
  if (!res.ok) throw new Error(await errorMessage(res, 'JMAP request failed'));

  const data = await res.json();
  // A method-level error (RFC 8620 §3.6.2) arrives with HTTP 200 and replaces
  // the arguments a caller expects, so without this every such failure looked
  // like an empty result and passed silently.
  const methodError = findMethodError(data);
  if (methodError) throw methodError;
  return data;
}

// Internal alias — the rest of this module was written against `post`.
const post = jmapPost;

/** Prefer the server's `detail` over a bare status code — it is user-readable. */
async function errorMessage(res, fallback) {
  try {
    const body = await res.json();
    if (body?.detail) return body.detail;
  } catch {
    // Non-JSON error body; fall through to the generic message.
  }
  if (res.status === 429) return 'Too many requests — please slow down.';
  if (res.status === 504) return 'The mail server is not responding.';
  if (res.status >= 500) return 'The mail server returned an error.';
  return `${fallback} (${res.status})`;
}

export async function getJMAPSession() {
  const res = await apiFetch('/api/jmap/session');
  if (!res) return null;
  if (!res.ok) throw new Error(await errorMessage(res, 'Could not load your mailbox'));
  return res.json();
}

export async function getAppConfig() {
  const res = await fetch('/api/config');
  if (!res.ok) return { appName: 'MeCloud' };
  return res.json();
}

export async function getMe() {
  const res = await apiFetch('/auth/me');
  if (!res) return { authenticated: false };
  if (!res.ok) return { authenticated: false };
  return res.json();
}

export async function logout() {
  // POST, not a link: a GET logout endpoint can be fired cross-site by any
  // <img src="…/auth/logout"> on a page the user happens to visit.
  try {
    await apiFetch('/auth/logout', { method: 'POST' });
  } catch {
    // Even if the call fails, sending the user to a fresh page is the right
    // end state — the cookie is httpOnly and will be re-validated there.
  }
  window.location.href = '/';
}

export async function getIdentities(accountId) {
  const data = await post(
    [['Identity/get', { accountId, ids: null }, 'id']],
    ['urn:ietf:params:jmap:core', 'urn:ietf:params:jmap:submission']
  );
  return data?.methodResponses?.[0]?.[1]?.list ?? [];
}

export function parseAddresses(str) {
  if (!str?.trim()) return [];
  return str.split(',').map(s => {
    s = s.trim();
    const m = s.match(/^(.+?)\s*<([^>]+)>$/);
    return m ? { name: m[1].trim(), email: m[2].trim() } : { email: s };
  }).filter(a => a.email);
}

/**
 * RFC 5322 headers that make a reply thread correctly in every other mail
 * client. Without them a reply starts a brand-new conversation, which is the
 * single most visible way a webmail client feels unfinished.
 */
export function threadingHeaders(inReplyTo, references) {
  const headers = {};
  if (!inReplyTo) return headers;
  headers.inReplyTo = [inReplyTo];
  // References is the full ancestry, capped so a long thread cannot produce an
  // unbounded header.
  const chain = [...(references ?? []), inReplyTo].filter(Boolean);
  headers.references = chain.slice(-20);
  return headers;
}

/**
 * Turn the compose modal's attachment rows into JMAP EmailBodyPart values.
 *
 * Only rows whose upload finished carry a blobId; anything else is dropped
 * rather than sent as an empty part.
 */
export function attachmentParts(attachments) {
  return (attachments ?? [])
    .filter((a) => a?.blobId)
    .map((a) => ({
      blobId: a.blobId,
      type: a.type || 'application/octet-stream',
      name: a.name || 'attachment',
      disposition: 'attachment',
      ...(Number.isFinite(Number(a.size)) ? { size: Number(a.size) } : {}),
    }));
}

export async function sendEmail(accountId, identityId, {
  fromEmail, fromName, to, cc, bcc, subject, html, sentMailboxId,
  inReplyTo = null, references = null, attachments = null,
}) {
  const from = [fromName ? { name: fromName, email: fromEmail } : { email: fromEmail }];
  const toAddrs = parseAddresses(to);
  const ccAddrs = parseAddresses(cc);
  const bccAddrs = parseAddresses(bcc);

  const emailCreate = {
    from,
    to: toAddrs,
    subject: subject || '(no subject)',
    keywords: { '$seen': true },
    bodyValues: { body: { value: html || '' } },
    htmlBody: [{ partId: 'body', type: 'text/html' }],
    ...threadingHeaders(inReplyTo, references),
  };
  if (sentMailboxId) emailCreate.mailboxIds = { [sentMailboxId]: true };
  if (ccAddrs.length) emailCreate.cc = ccAddrs;
  if (bccAddrs.length) emailCreate.bcc = bccAddrs;

  // Attachments are references, not bytes: the blob is uploaded first and the
  // message only names it. Blob ids are account-scoped in JMAP, which is what
  // lets a file already in the drive be attached without re-uploading it.
  const parts = attachmentParts(attachments);
  if (parts.length) emailCreate.attachments = parts;

  // Step 1: create the email
  const createData = await post(
    [['Email/set', { accountId, create: { draft: emailCreate } }, 'e']],
    ['urn:ietf:params:jmap:core', 'urn:ietf:params:jmap:mail']
  );

  const emailResp = createData?.methodResponses?.[0]?.[1];
  if (emailResp?.notCreated?.draft) {
    throw new JmapSetError(emailResp.notCreated.draft, 'Failed to create email');
  }

  const emailId = emailResp?.created?.draft?.id;
  if (!emailId) throw new Error('Email was created but server returned no ID');

  // Step 2: submit using the concrete emailId
  const submitData = await post(
    [['EmailSubmission/set', { accountId, create: { send: { identityId, emailId } } }, 's']],
    ['urn:ietf:params:jmap:core', 'urn:ietf:params:jmap:mail', 'urn:ietf:params:jmap:submission']
  );

  const submitResp = submitData?.methodResponses?.[0]?.[1];
  if (submitResp?.notCreated?.send) {
    throw new JmapSetError(submitResp.notCreated.send, 'Failed to submit email');
  }

  return { emailId };
}

/** What desktop artefacts this server actually shipped. */
export async function getDesktopReleases() {
  const res = await fetch('/api/desktop/releases', { credentials: 'include' });
  if (!res.ok) throw new Error('Could not check for desktop downloads.');
  const data = await res.json();
  return data?.artifacts ?? [];
}

/**
 * Start a download.
 *
 * The session buys a short-lived signed link rather than the bytes, so the
 * transfer itself is an ordinary browser download — it survives leaving the
 * page and shows up in the download manager, which a fetch() into memory
 * would not.
 */
export async function downloadDesktopArtifact(key) {
  const res = await fetch(`/api/desktop/token?artifact=${encodeURIComponent(key)}`, {
    method: 'POST',
    credentials: 'include',
  });
  if (!res.ok) throw new Error('Could not start that download.');
  const { url, token } = await res.json();

  const a = document.createElement('a');
  a.href = `${url}?token=${encodeURIComponent(token)}`;
  a.rel = 'noopener';
  document.body.appendChild(a);
  a.click();
  a.remove();
}

export async function getMailboxes(accountId) {
  const data = await post([
    ['Mailbox/get', { accountId, ids: null }, 'mb']
  ]);
  return data?.methodResponses?.[0]?.[1]?.list ?? [];
}

/**
 * Just the counters, for the background refresh. Asking for three properties
 * instead of the whole Mailbox object cuts the polled payload by roughly an
 * order of magnitude on an account with many folders.
 */
export async function getMailboxCounts(accountId) {
  const data = await post([
    ['Mailbox/get', {
      accountId,
      ids: null,
      properties: ['id', 'unreadEmails', 'totalEmails'],
    }, 'mb']
  ]);
  return data?.methodResponses?.[0]?.[1]?.list ?? [];
}

// Everything the message list renders, and nothing else.
const LIST_PROPERTIES = [
  'id', 'threadId', 'subject', 'from', 'receivedAt', 'preview', 'keywords', 'hasAttachment',
];

export async function getEmails(accountId, mailboxId, position = 0, limit = 50) {
  const data = await post([
    [
      'Email/query',
      {
        accountId,
        filter: { inMailbox: mailboxId },
        sort: [{ property: 'receivedAt', isAscending: false }],
        position,
        limit,
        calculateTotal: false,
      },
      'q'
    ],
    [
      'Email/get',
      {
        accountId,
        '#ids': { resultOf: 'q', name: 'Email/query', path: '/ids' },
        properties: LIST_PROPERTIES,
      },
      'e'
    ]
  ]);
  return data?.methodResponses?.[1]?.[1]?.list ?? [];
}

export async function searchEmails(accountId, query, position = 0, limit = 50) {
  const data = await post([
    [
      'Email/query',
      {
        accountId,
        filter: { text: query },
        sort: [{ property: 'receivedAt', isAscending: false }],
        position,
        limit,
        calculateTotal: false,
      },
      'q'
    ],
    [
      'Email/get',
      {
        accountId,
        '#ids': { resultOf: 'q', name: 'Email/query', path: '/ids' },
        properties: LIST_PROPERTIES,
      },
      'e'
    ]
  ]);
  return data?.methodResponses?.[1]?.[1]?.list ?? [];
}

export async function destroyEmail(accountId, emailId) {
  const data = await post([
    ['Email/set', { accountId, destroy: [emailId] }, 'del']
  ]);
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notDestroyed?.[emailId]) {
    throw new JmapSetError(resp.notDestroyed[emailId], 'Delete failed');
  }
}

export async function moveEmail(accountId, emailId, toMailboxId, fromMailboxId) {
  const patch = { [`mailboxIds/${toMailboxId}`]: true };
  if (fromMailboxId) patch[`mailboxIds/${fromMailboxId}`] = null;
  const data = await post([
    ['Email/set', { accountId, update: { [emailId]: patch } }, 'mv']
  ]);
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notUpdated?.[emailId]) {
    throw new JmapSetError(resp.notUpdated[emailId], 'Move failed');
  }
}

export async function renameMailbox(accountId, mailboxId, newName) {
  const data = await post([
    ['Mailbox/set', { accountId, update: { [mailboxId]: { name: newName } } }, 'mb']
  ]);
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notUpdated?.[mailboxId]) {
    throw new JmapSetError(resp.notUpdated[mailboxId], 'Rename failed');
  }
}

export async function deleteMailbox(accountId, mailboxId) {
  const data = await post([
    ['Mailbox/set', { accountId, destroy: [mailboxId] }, 'mb']
  ]);
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notDestroyed?.[mailboxId]) {
    throw new JmapSetError(resp.notDestroyed[mailboxId], 'Delete failed');
  }
}

export async function createMailbox(accountId, name, parentId = null) {
  const props = { name, role: null };
  if (parentId) props.parentId = parentId;
  const data = await post([
    ['Mailbox/set', { accountId, create: { newMailbox: props } }, 'mb']
  ]);
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notCreated?.newMailbox) {
    throw new JmapSetError(resp.notCreated.newMailbox, 'Create failed');
  }
  const created = resp?.created?.newMailbox ?? null;
  if (!created) return null;
  // JMAP only returns server-assigned fields in `created`; merge with what we sent
  return { ...props, ...created };
}

export async function bulkMarkSeen(accountId, emailIds, seen) {
  const update = {};
  for (const id of emailIds) update[id] = { 'keywords/$seen': seen ? true : null };
  const data = await post([['Email/set', { accountId, update }, 'mark']]);
  // This used to ignore the response entirely, so a partial failure left the
  // UI showing messages as read that the server had refused to change.
  const resp = data?.methodResponses?.[0]?.[1];
  const failed = resp?.notUpdated ? Object.keys(resp.notUpdated) : [];
  if (failed.length > 0) {
    const first = resp.notUpdated[failed[0]];
    throw new JmapSetError(first, `Could not update ${failed.length} message(s)`);
  }
}

export async function bulkDestroy(accountId, emailIds) {
  const data = await post([['Email/set', { accountId, destroy: emailIds }, 'del']]);
  const resp = data?.methodResponses?.[0]?.[1];
  const failed = resp?.notDestroyed ? Object.keys(resp.notDestroyed) : [];
  if (failed.length > 0) {
    const first = resp.notDestroyed[failed[0]];
    throw new JmapSetError(first, `Delete failed for ${failed.length} message(s)`);
  }
}

export async function bulkMove(accountId, emailIds, toMailboxId, fromMailboxId) {
  const update = {};
  for (const id of emailIds) {
    const patch = { [`mailboxIds/${toMailboxId}`]: true };
    if (fromMailboxId) patch[`mailboxIds/${fromMailboxId}`] = null;
    update[id] = patch;
  }
  const data = await post([['Email/set', { accountId, update }, 'mv']]);
  const resp = data?.methodResponses?.[0]?.[1];
  const failed = resp?.notUpdated ? Object.keys(resp.notUpdated) : [];
  if (failed.length > 0) {
    const first = resp.notUpdated[failed[0]];
    throw new JmapSetError(first, `Move failed for ${failed.length} message(s)`);
  }
}

export async function markEmailSeen(accountId, emailId, seen) {
  const data = await post([
    ['Email/set', { accountId, update: { [emailId]: { 'keywords/$seen': seen ? true : null } } }, 'mark']
  ]);
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notUpdated?.[emailId]) {
    throw new JmapSetError(resp.notUpdated[emailId], 'Mark failed');
  }
}

function appPasswordUsing(session) {
  // Use every capability the server advertises — avoids guessing the exact URI
  // for Stalwart's x:AppPassword extension.
  const caps = Object.keys(session?.capabilities ?? {});
  return caps.length ? caps : ['urn:ietf:params:jmap:core'];
}

export async function getAppPasswords(accountId, session) {
  const using = appPasswordUsing(session);
  const data = await post([
    ['x:AppPassword/query', { accountId }, '0'],
    ['x:AppPassword/get', {
      accountId,
      '#ids': { resultOf: '0', name: 'x:AppPassword/query', path: '/ids' },
    }, '1'],
  ], using);
  return data?.methodResponses?.[1]?.[1]?.list ?? [];
}

export async function createAppPassword(accountId, description, session) {
  const using = appPasswordUsing(session);
  const data = await post([
    ['x:AppPassword/set', {
      accountId,
      create: { new: { description, expiresAt: null } },
    }, '0'],
  ], using);
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notCreated?.new) {
    throw new JmapSetError(resp.notCreated.new, 'Failed to create app password');
  }
  return resp?.created?.new ?? null; // { id, secret, ... }
}

export async function deleteAppPassword(accountId, id, session) {
  const using = appPasswordUsing(session);
  const data = await post([
    ['x:AppPassword/set', { accountId, destroy: [id] }, '0'],
  ], using);
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notDestroyed?.[id]) {
    throw new JmapSetError(resp.notDestroyed[id], 'Failed to delete app password');
  }
}

// ── Calendar (RFC 8984 / urn:ietf:params:jmap:calendars) ────────────────────

function calUsing(session) {
  const caps = Object.keys(session?.capabilities ?? {});
  return caps.length ? caps : ['urn:ietf:params:jmap:core', 'urn:ietf:params:jmap:calendars'];
}

export async function getCalendars(accountId, session) {
  const data = await post(
    [['Calendar/get', { accountId, ids: null }, 'c']],
    calUsing(session)
  );
  return data?.methodResponses?.[0]?.[1]?.list ?? [];
}

export async function getCalendarEvents(accountId, session, calendarId, after, before) {
  const filter = {};
  if (calendarId) filter.calendarIds = { [calendarId]: true };
  if (after)  filter.after  = after;
  if (before) filter.before = before;

  const data = await post([
    ['CalendarEvent/query', { accountId, filter, limit: 256 }, 'q'],
    ['CalendarEvent/get',  {
      accountId,
      '#ids': { resultOf: 'q', name: 'CalendarEvent/query', path: '/ids' },
    }, 'e'],
  ], calUsing(session));
  return data?.methodResponses?.[1]?.[1]?.list ?? [];
}

export async function createCalendarEvent(accountId, session, event) {
  const data = await post(
    [['CalendarEvent/set', { accountId, create: { new: event } }, 'e']],
    calUsing(session)
  );
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notCreated?.new) throw new JmapSetError(resp.notCreated.new, 'Create failed');
  return resp?.created?.new ?? null;
}

export async function updateCalendarEvent(accountId, session, eventId, patch) {
  const data = await post(
    [['CalendarEvent/set', { accountId, update: { [eventId]: patch } }, 'e']],
    calUsing(session)
  );
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notUpdated?.[eventId]) throw new JmapSetError(resp.notUpdated[eventId], 'Update failed');
}

export async function deleteCalendarEvent(accountId, session, eventId) {
  const data = await post(
    [['CalendarEvent/set', { accountId, destroy: [eventId] }, 'e']],
    calUsing(session)
  );
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notDestroyed?.[eventId]) throw new JmapSetError(resp.notDestroyed[eventId], 'Delete failed');
}

export async function createCalendar(accountId, session, name) {
  const data = await post(
    [['Calendar/set', { accountId, create: { new: { name, isSubscribed: true } } }, 'c']],
    calUsing(session)
  );
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notCreated?.new) throw new JmapSetError(resp.notCreated.new, 'Create failed');
  return resp?.created?.new ?? null;
}

export async function deleteCalendar(accountId, session, calendarId) {
  const data = await post(
    [['Calendar/set', { accountId, destroy: [calendarId] }, 'c']],
    calUsing(session)
  );
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notDestroyed?.[calendarId]) throw new JmapSetError(resp.notDestroyed[calendarId], 'Delete failed');
}

// ── Contacts (RFC 9553 / urn:ietf:params:jmap:contacts) ─────────────────────

function contactUsing(session) {
  const caps = Object.keys(session?.capabilities ?? {});
  return caps.length ? caps : ['urn:ietf:params:jmap:core', 'urn:ietf:params:jmap:contacts'];
}

export async function getAddressBooks(accountId, session) {
  const data = await post(
    [['AddressBook/get', { accountId, ids: null }, 'a']],
    contactUsing(session)
  );
  return data?.methodResponses?.[0]?.[1]?.list ?? [];
}

export async function getContacts(accountId, session, addressBookId) {
  const filter = addressBookId ? { addressBookId } : {};
  const data = await post([
    ['ContactCard/query', { accountId, filter, limit: 500 }, 'q'],
    ['ContactCard/get',  {
      accountId,
      '#ids': { resultOf: 'q', name: 'ContactCard/query', path: '/ids' },
    }, 'c'],
  ], contactUsing(session));
  return data?.methodResponses?.[1]?.[1]?.list ?? [];
}

export async function createContact(accountId, session, contact, addressBookId) {
  const card = addressBookId ? { ...contact, addressBookIds: { [addressBookId]: true } } : contact;
  const data = await post(
    [['ContactCard/set', { accountId, create: { new: card } }, 'c']],
    contactUsing(session)
  );
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notCreated?.new) throw new JmapSetError(resp.notCreated.new, 'Create failed');
  return resp?.created?.new ?? null;
}

export async function updateContact(accountId, session, contactId, patch) {
  const data = await post(
    [['ContactCard/set', { accountId, update: { [contactId]: patch } }, 'c']],
    contactUsing(session)
  );
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notUpdated?.[contactId]) throw new JmapSetError(resp.notUpdated[contactId], 'Update failed');
}

export async function deleteContact(accountId, session, contactId) {
  const data = await post(
    [['ContactCard/set', { accountId, destroy: [contactId] }, 'c']],
    contactUsing(session)
  );
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notDestroyed?.[contactId]) throw new JmapSetError(resp.notDestroyed[contactId], 'Delete failed');
}

export async function createAddressBook(accountId, session, name) {
  const data = await post(
    [['AddressBook/set', { accountId, create: { new: { name } } }, 'a']],
    contactUsing(session)
  );
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notCreated?.new) throw new JmapSetError(resp.notCreated.new, 'Create failed');
  return resp?.created?.new ?? null;
}

export async function deleteAddressBook(accountId, session, addressBookId) {
  const data = await post(
    [['AddressBook/set', { accountId, destroy: [addressBookId] }, 'a']],
    contactUsing(session)
  );
  const resp = data?.methodResponses?.[0]?.[1];
  if (resp?.notDestroyed?.[addressBookId]) throw new JmapSetError(resp.notDestroyed[addressBookId], 'Delete failed');
}

export async function getSieveScript() {
  const res = await apiFetch('/api/sieve');
  if (!res) return null;
  if (res.status === 503) return null;
  if (!res.ok) throw new Error(await errorMessage(res, 'Could not load your rules'));
  return res.json();
}

export async function saveSieveScript(id, name, content, makeActive = true) {
  const res = await apiFetch('/api/sieve', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ id, name, content, makeActive }),
  });
  if (!res) return null;
  if (!res.ok) throw new Error(await errorMessage(res, 'Could not save your rules'));
  return res.json();
}

export async function getEmailBody(accountId, emailId) {
  const data = await post([
    [
      'Email/get',
      {
        accountId,
        ids: [emailId],
        properties: [
          'id', 'threadId', 'subject', 'from', 'to', 'cc', 'replyTo',
          'receivedAt', 'keywords', 'hasAttachment', 'attachments',
          // messageId/references are what make a reply thread correctly.
          'messageId', 'references', 'inReplyTo',
          'htmlBody', 'textBody', 'bodyValues',
        ],
        fetchHTMLBodyValues: true,
        fetchTextBodyValues: true,
        maxBodyValueBytes: 1_000_000,   // don't pull a 50 MB body into the tab
      },
      'e'
    ]
  ]);
  return data?.methodResponses?.[0]?.[1]?.list?.[0] ?? null;
}
