/**
 * Dashboard summary.
 *
 * Mail counts, the contact total and today's agenda are all fetched in a
 * *single* JMAP request. The protocol takes an array of method calls per
 * request, so separate round trips would be separate latency for no extra
 * information — and on a phone that difference is the whole feel of the page.
 *
 * Two things cannot ride in that one request, and both are deliberate:
 *
 *   Files, because `FileNode/get` with `ids: null` silently truncates at
 *   maxObjectsInGet (500 on this server), which made every count here wrong on
 *   a drive bigger than that. Enumeration pages instead — see files.js — so it
 *   is several round trips, run alongside the batch rather than after it.
 *
 *   Unread messages, because the filter needs the Inbox's id and only the
 *   Mailbox/get in the batch can supply it. A JMAP result reference replaces a
 *   whole argument (RFC 8620 section 3.7); it cannot be spliced into a field
 *   *inside* the filter object, so this one genuinely has to come second.
 */
import { jmapPost } from './api.js';
import { fetchFileNodes } from './files.js';
import { fileKind } from './fileTypes.js';
import { isMarkdown } from './markdown.js';
import { findNotesFolder, collectNotes } from './notes.js';

/** How many rows each list on the overview shows. */
export const UNREAD_LIMIT = 8;
export const RECENT_FILES_LIMIT = 5;
export const RECENT_NOTES_LIMIT = 5;

/**
 * All the overview reads off a FileNode. Asking for these rather than
 * everything keeps a multi-page enumeration to what is actually displayed.
 */
const FILE_PROPERTIES = ['id', 'name', 'parentId', 'blobId', 'size', 'type', 'modified'];

const UNREAD_PROPERTIES = ['id', 'subject', 'from', 'receivedAt', 'preview', 'hasAttachment'];

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

/** Newest first, treating a missing timestamp as the epoch rather than NaN. */
function byNewest(key) {
  return (a, b) => new Date(b?.[key] ?? 0) - new Date(a?.[key] ?? 0);
}

// ── Files and notes ─────────────────────────────────────────────────────────

/**
 * Every Markdown file that counts as a note.
 *
 * Under the Notes folder when there is one; anywhere on the drive when there
 * is not, so an account that has never opened the Notes app still shows what
 * it has rather than claiming zero.
 */
function allNotes(nodes) {
  const folder = findNotesFolder(nodes);
  return folder ? collectNotes(nodes, folder.id) : nodes.filter(isMarkdown);
}

/** Every id at or below `rootId`. Cycle-safe: a node already seen is not re-walked. */
function subtreeIds(nodes, rootId) {
  const ids = new Set();
  if (!rootId) return ids;

  const byParent = new Map();
  for (const node of nodes) {
    const key = node.parentId ?? null;
    if (!byParent.has(key)) byParent.set(key, []);
    byParent.get(key).push(node);
  }

  const walk = (parentId) => {
    for (const child of byParent.get(parentId) ?? []) {
      if (ids.has(child.id)) continue;
      ids.add(child.id);
      walk(child.id);
    }
  };
  walk(rootId);
  return ids;
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
  return { files, folders, bytes, notes: allNotes(nodes).length };
}

/**
 * The most recently changed files, newest first.
 *
 * Folders are excluded because "recent" here means something you worked on,
 * and a folder's timestamp moves whenever anything lands inside it. The Notes
 * subtree is excluded too — it has its own section below, and a note appearing
 * in both reads as a duplicate rather than as two facts.
 */
export function recentFiles(nodes, { limit = RECENT_FILES_LIMIT } = {}) {
  const notesFolder = findNotesFolder(nodes);
  const skip = subtreeIds(nodes, notesFolder?.id ?? null);

  return nodes
    .filter((node) => fileKind(node) !== 'folder' && !skip.has(node.id))
    .sort(byNewest('modified'))
    .slice(0, limit);
}

/** The most recently changed notes, newest first. */
export function recentNotes(nodes, { limit = RECENT_NOTES_LIMIT } = {}) {
  return [...allNotes(nodes)].sort(byNewest('modified')).slice(0, limit);
}

// ── Mail ────────────────────────────────────────────────────────────────────

/** Roll mailboxes up into totals plus the handful worth showing by name. */
export function summariseMail(mailboxes) {
  const byRole = (role) => mailboxes.find((m) => m.role === role) ?? null;
  const inbox = byRole('inbox');
  return {
    inboxId: inbox?.id ?? null,
    unread: inbox?.unreadEmails ?? 0,
    inboxTotal: inbox?.totalEmails ?? 0,
    total: mailboxes.reduce((n, m) => n + (Number(m.totalEmails) || 0), 0),
    unreadAll: mailboxes.reduce((n, m) => n + (Number(m.unreadEmails) || 0), 0),
    drafts: byRole('drafts')?.totalEmails ?? 0,
    mailboxes: mailboxes.length,
  };
}

/** Who a message is from, as one line: a name if there is one, else the address. */
export function senderLabel(email) {
  const from = email?.from?.[0];
  return from?.name?.trim() || from?.email?.trim() || 'Unknown sender';
}

/**
 * The newest unread messages in the Inbox.
 *
 * Only the Inbox: unread junk and unread archive are not things to be nudged
 * about, and counting them here would disagree with the number on the tile.
 */
export async function getUnreadMessages(accountId, session, inboxId, { limit = UNREAD_LIMIT } = {}) {
  if (!inboxId) return [];

  const data = await jmapPost([
    ['Email/query', {
      accountId,
      filter: { inMailbox: inboxId, notKeyword: '$seen' },
      sort: [{ property: 'receivedAt', isAscending: false }],
      position: 0,
      limit,
      calculateTotal: false,
    }, 'uq'],
    ['Email/get', {
      accountId,
      '#ids': { resultOf: 'uq', name: 'Email/query', path: '/ids' },
      properties: UNREAD_PROPERTIES,
    }, 'ug'],
  ], allCapabilities(session));

  // /get is not obliged to preserve /query's order (RFC 8620 section 5.1), and
  // a list captioned "newest first" that is not would be a quiet lie.
  return [...(data?.methodResponses?.[1]?.[1]?.list ?? [])].sort(byNewest('receivedAt'));
}

// ── The whole overview ──────────────────────────────────────────────────────

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

  // Started together, not in sequence: the drive enumeration is several round
  // trips and there is no reason for the batch to wait behind it.
  const [data, drive] = await Promise.all([
    jmapPost(calls, allCapabilities(session)),
    hasFiles ? fetchFileNodes(accountId, session, { properties: FILE_PROPERTIES })
             : Promise.resolve(null),
  ]);

  const responses = new Map(
    (data?.methodResponses ?? []).map(([, payload, callId]) => [callId, payload]),
  );

  const mailboxes = responses.get('mb')?.list ?? [];
  const mail = summariseMail(mailboxes);
  const events = (responses.get('eg')?.list ?? [])
    .filter((event) => isToday(event, now))
    .sort(compareEvents);

  // The unread list is illustration; the counts above it are the substance.
  // A failure here leaves the section saying so rather than taking the page
  // down, and `null` is distinguishable from a genuinely empty inbox.
  const unread = await getUnreadMessages(accountId, session, mail.inboxId).catch(() => null);

  const nodes = drive?.nodes ?? [];

  return {
    mail,
    unread,
    contacts: hasContacts ? (responses.get('cq')?.total ?? 0) : null,
    events: hasCalendar ? events : null,
    files: hasFiles ? { ...summariseFiles(nodes), incomplete: drive?.incomplete ?? false } : null,
    recentFiles: hasFiles ? recentFiles(nodes) : null,
    recentNotes: hasFiles ? recentNotes(nodes) : null,
    capabilities: { calendar: hasCalendar, contacts: hasContacts, files: hasFiles },
  };
}
