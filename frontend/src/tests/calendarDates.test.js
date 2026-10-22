import { describe, it, expect } from 'vitest';
import {
  localDateString, parseLocalDate, addDays, daysBetween, durationDays,
  allDayDuration, parseEventStart, buildEventPatch,
} from '$lib/calendarDates.js';
import { isToday } from '$lib/dashboard.js';

// The test runner's zone is whatever the container says; these assertions
// hold in every zone because nothing here may go through UTC.

describe('local calendar dates', () => {
  it('formats the local day, not the UTC one', () => {
    // 00:30 local is the previous day in UTC anywhere east of Greenwich.
    expect(localDateString(new Date(2026, 2, 10, 0, 30))).toBe('2026-03-10');
    expect(localDateString(new Date(2026, 2, 10, 23, 30))).toBe('2026-03-10');
  });

  it('parses a date as local midnight', () => {
    const d = parseLocalDate('2026-03-10');
    expect([d.getFullYear(), d.getMonth(), d.getDate(), d.getHours()]).toEqual([2026, 2, 10, 0]);
    expect(parseLocalDate('2026-03-10T09:00:00')).toBeNull();
  });

  it('adds days across month ends and DST changes', () => {
    expect(addDays('2026-03-31', 1)).toBe('2026-04-01');
    expect(addDays('2026-03-28', 2)).toBe('2026-03-30');   // EU DST starts 29 March
    expect(addDays('2026-11-01', -1)).toBe('2026-10-31');
  });

  it('counts whole days', () => {
    expect(daysBetween('2026-03-28', '2026-03-30')).toBe(2);
    expect(daysBetween('2026-03-10', '2026-03-10')).toBe(0);
  });

  it('reads all-day durations', () => {
    expect(durationDays('P2D')).toBe(2);
    expect(durationDays('P1W')).toBe(7);
    expect(durationDays('PT1H')).toBe(1);
    expect(durationDays(undefined)).toBe(1);
  });

  it('round-trips a two-day event without shrinking it', () => {
    const start = '2026-03-10';
    const end = addDays(start, durationDays('P2D') - 1);
    expect(end).toBe('2026-03-11');
    expect(allDayDuration(start, end)).toBe('P2D');
  });

  it('parses event starts, date-only as local', () => {
    expect(parseEventStart('2026-03-10').getDate()).toBe(10);
    expect(parseEventStart('2026-03-10T09:00:00').getHours()).toBe(9);
    expect(parseEventStart('nonsense')).toBeNull();
  });
});

describe('isToday with all-day events', () => {
  const now = new Date(2026, 2, 10, 12);

  it('shows an all-day event on its own day', () => {
    expect(isToday({ start: '2026-03-10', duration: 'P1D' }, now)).toBe(true);
  });

  it("does not show yesterday's all-day event today", () => {
    expect(isToday({ start: '2026-03-09', duration: 'P1D' }, now)).toBe(false);
  });
});

describe('buildEventPatch', () => {
  const form = {
    title: ' Trip ', allDay: true, date: '2026-03-10', endDate: '2026-03-11',
    startTime: '09:00', timezone: 'Europe/Berlin', calendarId: 'c1',
    location: '', description: '', status: 'confirmed', recurrence: 'none',
    timedDuration: null,
  };

  it('nulls cleared optional fields on update, so the server drops them', () => {
    const patch = buildEventPatch(form, { isUpdate: true });
    expect(patch.locations).toBeNull();
    expect(patch.description).toBeNull();
    expect(patch.recurrenceRules).toBeNull();
    expect(patch.timeZone).toBeNull();
    expect(patch.duration).toBe('P2D');
    expect(patch.title).toBe('Trip');
  });

  it('leaves them out on create', () => {
    const patch = buildEventPatch(form);
    expect('locations' in patch).toBe(false);
    expect('timeZone' in patch).toBe(false);
  });

  it('keeps filled fields', () => {
    const patch = buildEventPatch({
      ...form, allDay: false, location: 'Office', recurrence: 'weekly', timedDuration: 'PT1H',
    }, { isUpdate: true });
    expect(patch.locations.l1.name).toBe('Office');
    expect(patch.recurrenceRules[0].frequency).toBe('weekly');
    expect(patch.timeZone).toBe('Europe/Berlin');
    expect(patch.start).toBe('2026-03-10T09:00:00');
  });
});
