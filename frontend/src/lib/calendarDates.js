/**
 * Calendar date helpers.
 *
 * All-day events are *dates*, not instants: "10 March" means the same day in
 * every time zone. The bug these replace was formatting a local-midnight Date
 * with toISOString(), which converts to UTC first — east of UTC that is the
 * previous evening, so a click on 10 March made an event on 9 March and
 * re-saving a two-day event shrank it to one. Everything here reads and writes
 * the local calendar fields instead.
 */

const pad = (n) => String(n).padStart(2, '0');

/** `YYYY-MM-DD` for the local calendar day of `d`. */
export function localDateString(d) {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** Local midnight of a `YYYY-MM-DD` string, or null if it is not one. */
export function parseLocalDate(str) {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(String(str ?? ''));
  if (!m) return null;
  return new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
}

/** `YYYY-MM-DD` plus `n` calendar days (DST-safe: it moves the date, not ms). */
export function addDays(dateStr, n) {
  const d = parseLocalDate(dateStr);
  if (!d) return dateStr;
  d.setDate(d.getDate() + n);
  return localDateString(d);
}

/** Whole calendar days from `a` to `b` (both `YYYY-MM-DD`). */
export function daysBetween(a, b) {
  const da = parseLocalDate(a);
  const db = parseLocalDate(b);
  if (!da || !db) return 0;
  // Date.UTC on the local fields: no DST hour can skew the division.
  const ua = Date.UTC(da.getFullYear(), da.getMonth(), da.getDate());
  const ub = Date.UTC(db.getFullYear(), db.getMonth(), db.getDate());
  return Math.round((ub - ua) / 86400000);
}

/** Number of days in an all-day duration (`P2D`, `P1W`). At least 1. */
export function durationDays(iso) {
  const m = /^P(?:(\d+)W)?(?:(\d+)D)?/.exec(String(iso ?? ''));
  if (!m) return 1;
  const days = Number(m[1] ?? 0) * 7 + Number(m[2] ?? 0);
  return days > 0 ? days : 1;
}

/** `P<n>D` covering `start` through `end` inclusive. */
export function allDayDuration(start, end) {
  return `P${Math.max(1, daysBetween(start, end || start) + 1)}D`;
}

/**
 * An event `start` as a Date. Date-only values are local midnight, not UTC
 * midnight — `new Date('2026-03-10')` is the latter, which shows an all-day
 * event on the wrong day anywhere west of Greenwich.
 */
export function parseEventStart(start) {
  if (!start) return null;
  const local = parseLocalDate(start);
  if (local) return local;
  const d = new Date(start);
  return Number.isNaN(d.getTime()) ? null : d;
}

/**
 * The CalendarEvent properties a save should send.
 *
 * JMAP `/set` update is a *patch*: a property left out is left alone on the
 * server. So an optional field the user cleared must be sent as null, or the
 * old location, description or repeat rule silently survives the edit.
 * Creation leaves them out instead, which is the same thing for a new object.
 */
export function buildEventPatch(form, { isUpdate = false } = {}) {
  const event = {
    '@type': 'Event',
    title: form.title.trim(),
    start: form.allDay ? form.date : `${form.date}T${form.startTime}:00`,
    duration: form.allDay ? allDayDuration(form.date, form.endDate) : form.timedDuration,
    calendarIds: { [form.calendarId]: true },
  };
  const optional = {
    timeZone: form.allDay ? null : form.timezone,
    locations: form.location?.trim()
      ? { l1: { '@type': 'Location', name: form.location.trim() } } : null,
    description: form.description?.trim() || null,
    status: form.status && form.status !== 'confirmed' ? form.status : null,
    recurrenceRules: form.recurrence && form.recurrence !== 'none'
      ? [{ '@type': 'RecurrenceRule', frequency: form.recurrence }] : null,
  };
  for (const [key, value] of Object.entries(optional)) {
    if (value !== null) event[key] = value;
    // null in a patch resets the property to its default (RFC 8620 §5.3).
    else if (isUpdate) event[key] = null;
  }
  return event;
}
