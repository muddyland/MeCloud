import { writable } from 'svelte/store';

export const calendars        = writable([]);
export const calendarEvents   = writable([]);
export const selectedCalendar = writable(null); // null = all calendars
export const calendarLoading  = writable(false);
export const calendarView     = writable('month'); // 'month' | 'week'
export const calendarDate     = writable(new Date()); // current display date
export const eventModalOpen      = writable(false);
export const editingEvent        = writable(null); // null = new event
export const newEventDate        = writable(null); // pre-fill date when clicking a day
export const calendarRefreshToken = writable(0);   // increment to trigger a server reload
