/**
 * Dashboard summary.
 *
 * Everything the overview needs is fetched in a *single* JMAP request. The
 * protocol takes an array of method calls per request, so five separate
 * round trips would be five times the latency for no extra information — and
 * on a phone that difference is the whole feel of the page.
 */
import { jmapPost } from './api.js';
import { fileKind } from './fileTypes.js';
import { isMarkdown } from './markdown.js';
import { findNotesFolder, collectNotes } from './notes.js';

function allCapabilities(session) {
  const caps = Object.keys(session?.capabilities ?? {});
  return caps.length ? caps : ['urn:ietf:params:jmap:core'];
}

/** Local midnight today and tomorrow, as JMAP UTCDate strings. */
export function todayBounds(now = new Date()) {
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const end = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1);
  return { after: start.toISOString(), before: end.toISOString() };
}

/**
 * An event is "today" if it overlaps today at all — not merely if it starts
 * today. A multi-day trip should still show on its middle days.
 */
export function isToday(event, now = new Date()) {
  const startOfDay = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const endOfDay = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1);

  const start = event?.start ? new Date(event.start) : null;
  if (!start || Number.isNaN(start.getTime())) return false;

  const end = eventEnd(event) ?? start;
  return start < endOfDay && end >= startOfDay;
}

/** Event end, derived from `duration` when the server does not send `end`. */
export function eventEnd(event) {
  if (event?.end) {
    const end = new Date(event.end);
    if (!Number.isNaN(end.getTime())) return end;
  }
  const start = event?.start ? new Date(event.start) : null;
  if (!start || Number.isNaN(start.getTime())) return null;

  // ISO-8601 duration, e.g. PT1H30M or P1D — the subset JMAP actually emits.
  const match = /^P(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?)?$/
    .exec(String(event?.duration ?? ''));
  if (!match) return start;
  const [, d = 0, h = 0, m = 0, s = 0] = match;
  return new Date(start.getTime()
    + Number(d) * 86400000 + Number(h) * 3600000 + Number(m) * 60000 + Number(s) * 1000);
}

/** Sort key for the agenda: earliest first, all-day entries before timed ones. */
export function compareEvents(a, b) {
  const at = new Date(a?.start ?? 0).getTime();
  const bt = new Date(b?.start ?? 0).getTime();
  if (a?.showWithoutTime !== b?.showWithoutTime) return a?.showWithoutTime ? -1 : 1;
  return at - bt;
}

/** Roll a flat FileNode list up into counts and a byte total. */
export function summariseFiles(nodes) {
  let files = 0;
  let folders = 0;
  let bytes = 0;
  for (const node of nodes) {
    if (fileKind(node) === 'folder') folders += 1;
    else {
      files += 1;
      bytes += Number(node.size) || 0;
    }
  }
  const notesFolder = findNotesFolder(nodes);
  return {
    files,
    folders,
    bytes,
    notes: notesFolder ? collectNotes(nodes, notesFolder.id).length
                       : nodes.filter(isMarkdown).length,
  };
}

/** Roll mailboxes up into totals plus the handful worth showing by name. */
export function summariseMail(mailboxes) {
  const byRole = (role) => mailboxes.find((m) => m.role === role) ?? null;
  const inbox = byRole('inbox');
  return {
    unread: inbox?.unreadEmails ?? 0,
    inboxTotal: inbox?.totalEmails ?? 0,
    total: mailboxes.reduce((n, m) => n + (Number(m.totalEmails) || 0), 0),
    unreadAll: mailboxes.reduce((n, m) => n + (Number(m.unreadEmails) || 0), 0),
    drafts: byRole('drafts')?.totalEmails ?? 0,
    mailboxes: mailboxes.length,
  };
}

/**
 * Fetch everything the dashboard shows.
 *
 * Capabilities the server does not advertise are skipped rather than requested
 * and failed — asking for CalendarEvent on a server without calendars fails the
 * *whole* batched request, taking the mail counts down with it.
 */
export async function getDashboardSummary(accountId, session, { now = new Date() } = {}) {
  const caps = session?.capabilities ?? {};
  const hasCalendar = 'urn:ietf:params:jmap:calendars' in caps;
  const hasContacts = 'urn:ietf:params:jmap:contacts' in caps;
  const hasFiles = 'urn:ietf:params:jmap:filenode' in caps;

  const { after, before } = todayBounds(now);
  const calls = [
    ['Mailbox/get', {
      accountId, ids: null,
      properties: ['id', 'name', 'role', 'unreadEmails', 'totalEmails'],
    }, 'mb'],
  ];

  if (hasContacts) {
    // limit 0 with calculateTotal: the count is all the dashboard needs, and
    // pulling every card to length an array would be wasteful.
    calls.push(['ContactCard/query', { accountId, filter: {}, limit: 0, calculateTotal: true }, 'cq']);
  }
  if (hasCalendar) {
    calls.push(['CalendarEvent/query', { accountId, filter: { after, before }, limit: 50 }, 'eq']);
    calls.push(['CalendarEvent/get', {
      accountId,
      '#ids': { resultOf: 'eq', name: 'CalendarEvent/query', path: '/ids' },
      properties: ['id', 'title', 'start', 'duration', 'end', 'showWithoutTime',
                   'locations', 'calendarIds'],
    }, 'eg']);
  }
  if (hasFiles) {
    calls.push(['FileNode/get', {
      accountId, ids: null,
      properties: ['id', 'name', 'parentId', 'blobId', 'size', 'type'],
    }, 'fn']);
  }

  const data = await jmapPost(calls, allCapabilities(session));
  const responses = new Map(
    (data?.methodResponses ?? []).map(([, payload, callId]) => [callId, payload]),
  );

  const mailboxes = responses.get('mb')?.list ?? [];
  const fileNodes = responses.get('fn')?.list ?? [];
  const events = (responses.get('eg')?.list ?? [])
    .filter((event) => isToday(event, now))
    .sort(compareEvents);

  return {
    mail: summariseMail(mailboxes),
    contacts: hasContacts ? (responses.get('cq')?.total ?? 0) : null,
    events: hasCalendar ? events : null,
    files: hasFiles ? summariseFiles(fileNodes) : null,
    capabilities: { calendar: hasCalendar, contacts: hasContacts, files: hasFiles },
  };
}
