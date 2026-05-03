<script>
  import { eventModalOpen, editingEvent, newEventDate, calendars, selectedCalendar, calendarRefreshToken } from '$lib/stores/calendar.js';
  import { jmapAccountId, jmapSession } from '$lib/stores/mail.js';
  import { createCalendarEvent, updateCalendarEvent, deleteCalendarEvent } from '$lib/api.js';
  import { toast } from '$lib/stores/toast.js';

  let saving   = false;
  let deleting = false;

  // Form state
  let title       = '';
  let location    = '';
  let description = '';
  let allDay      = false;
  let date        = '';
  let endDate     = '';
  let startTime   = '';
  let endTime     = '';
  let timezone    = '';
  let calendarId  = '';
  let status      = 'confirmed';
  let recurrence  = 'none';

  const localTz = typeof Intl !== 'undefined'
    ? Intl.DateTimeFormat().resolvedOptions().timeZone
    : 'UTC';

  let timezones = [];
  try { timezones = Intl.supportedValuesOf('timeZone'); }
  catch { timezones = ['UTC','America/New_York','America/Chicago','America/Denver',
    'America/Los_Angeles','America/Toronto','Europe/London','Europe/Paris',
    'Europe/Berlin','Europe/Amsterdam','Asia/Tokyo','Asia/Shanghai',
    'Asia/Kolkata','Asia/Dubai','Australia/Sydney','Pacific/Auckland']; }

  $: if ($eventModalOpen) init();

  function init() {
    const ev = $editingEvent;
    timezone = localTz;
    if (ev) {
      title       = ev.title ?? '';
      location    = Object.values(ev.locations ?? {})[0]?.name ?? '';
      description = ev.description ?? '';
      status      = ev.status ?? 'confirmed';
      calendarId  = Object.keys(ev.calendarIds ?? {})[0] ?? defaultCalId();
      timezone    = ev.timeZone ?? localTz;
      allDay      = !ev.start?.includes('T');

      const dtStr = ev.start ?? '';
      if (allDay) {
        date    = dtStr.slice(0, 10);
        endDate = addDays(date, parseDaysDuration(ev.duration ?? 'P1D') - 1);
        startTime = '09:00'; endTime = '10:00';
      } else {
        [date, startTime] = dtStr.split('T');
        startTime = startTime.slice(0, 5);
        endTime   = addDurationToTime(startTime, ev.duration ?? 'PT1H');
        endDate   = date;
      }

      const rr = (ev.recurrenceRules ?? [])[0];
      recurrence = rr?.frequency ?? 'none';
    } else {
      title = location = description = '';
      status = 'confirmed'; recurrence = 'none';
      allDay = false;
      const d = $newEventDate ?? new Date();
      date = endDate = fmtDate(d);
      startTime = '09:00'; endTime = '10:00';
      calendarId = defaultCalId();
    }
  }

  function defaultCalId() {
    return $selectedCalendar ?? $calendars[0]?.id ?? '';
  }

  function fmtDate(d) {
    return d.toISOString().slice(0, 10);
  }

  function addDays(dateStr, n) {
    const d = new Date(dateStr + 'T00:00:00');
    d.setDate(d.getDate() + n);
    return fmtDate(d);
  }

  function parseDaysDuration(iso) {
    const m = iso.match(/P(\d+)D/);
    return m ? parseInt(m[1]) : 1;
  }

  function addDurationToTime(timeStr, iso) {
    const mh = iso.match(/(\d+)H/); const mm = iso.match(/(\d+)M/);
    const hours = mh ? parseInt(mh[1]) : 0;
    const mins  = mm ? parseInt(mm[1]) : 0;
    const [h, mi] = timeStr.split(':').map(Number);
    const total = h * 60 + mi + hours * 60 + mins;
    return `${String(Math.floor(total / 60) % 24).padStart(2,'0')}:${String(total % 60).padStart(2,'0')}`;
  }

  function calcTimedDuration() {
    const [sh, sm] = startTime.split(':').map(Number);
    const [eh, em] = endTime.split(':').map(Number);
    let mins = (eh * 60 + em) - (sh * 60 + sm);
    if (mins <= 0) mins = 60;
    const h = Math.floor(mins / 60); const m = mins % 60;
    return `PT${h > 0 ? h + 'H' : ''}${m > 0 ? m + 'M' : ''}` || 'PT1H';
  }

  function calcAllDayDuration() {
    const start = new Date(date + 'T00:00:00');
    const end   = new Date((endDate || date) + 'T00:00:00');
    const days  = Math.max(1, Math.round((end - start) / 86400000) + 1);
    return `P${days}D`;
  }

  function close() {
    eventModalOpen.set(false);
    editingEvent.set(null);
    newEventDate.set(null);
  }

  async function save() {
    if (!title.trim()) return;
    saving = true;
    try {
      const start = allDay ? date : `${date}T${startTime}:00`;
      const event = {
        '@type': 'Event',
        title: title.trim(),
        start,
        duration: allDay ? calcAllDayDuration() : calcTimedDuration(),
        calendarIds: { [calendarId]: true },
      };

      if (!allDay) event.timeZone = timezone;
      if (location.trim())    event.locations   = { l1: { '@type': 'Location', name: location.trim() } };
      if (description.trim()) event.description = description.trim();
      if (status !== 'confirmed') event.status  = status;
      if (recurrence !== 'none')  event.recurrenceRules = [{ '@type': 'RecurrenceRule', frequency: recurrence }];

      if ($editingEvent) {
        await updateCalendarEvent($jmapAccountId, $jmapSession, $editingEvent.id, event);
        toast('Event updated', 'success');
      } else {
        await createCalendarEvent($jmapAccountId, $jmapSession, event);
        toast('Event created', 'success');
      }
      calendarRefreshToken.update(n => n + 1);
      close();
    } catch (e) {
      toast(e?.message ?? 'Save failed', 'error');
    } finally {
      saving = false;
    }
  }

  async function remove() {
    if (!$editingEvent) return;
    deleting = true;
    try {
      await deleteCalendarEvent($jmapAccountId, $jmapSession, $editingEvent.id);
      calendarRefreshToken.update(n => n + 1);
      toast('Event deleted', 'success');
      close();
    } catch (e) {
      toast(e?.message ?? 'Delete failed', 'error');
    } finally {
      deleting = false;
    }
  }

  const inputCls = 'text-sm px-3 py-2 rounded-lg border border-gray-200 dark:border-gray-600 bg-white dark:bg-gray-700 text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-blue-500';
  const labelCls = 'block text-xs font-medium text-gray-500 dark:text-gray-400 mb-1';
</script>

{#if $eventModalOpen}
  <div class="fixed inset-0 bg-black/40 z-40 flex items-center justify-center p-4"
       on:click|self={close} role="dialog" aria-modal="true">

    <div class="w-full max-w-lg bg-white dark:bg-gray-800 rounded-2xl shadow-xl z-50 flex flex-col max-h-[90vh]">

      <!-- Header -->
      <div class="flex items-center justify-between px-5 py-4 border-b border-gray-200 dark:border-gray-700 flex-shrink-0">
        <h2 class="text-base font-semibold text-gray-900 dark:text-gray-100">
          {$editingEvent ? 'Edit Event' : 'New Event'}
        </h2>
        <button on:click={close} class="text-gray-400 hover:text-gray-600 dark:hover:text-gray-200">
          <svg class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M18 6 6 18M6 6l12 12"/></svg>
        </button>
      </div>

      <div class="overflow-y-auto flex-1 px-5 py-4 space-y-4">

        <!-- Title -->
        <input bind:value={title} placeholder="Event title"
          class="w-full {inputCls} text-base font-medium" />

        <!-- All-day -->
        <label class="flex items-center gap-2 text-sm text-gray-700 dark:text-gray-300 cursor-pointer select-none">
          <input type="checkbox" bind:checked={allDay} class="rounded" />
          All day
        </label>

        <!-- Date / time row -->
        {#if allDay}
          <div class="grid grid-cols-2 gap-2">
            <div>
              <label for="ev-date" class={labelCls}>Start date</label>
              <input id="ev-date" type="date" bind:value={date} class="w-full {inputCls}" />
            </div>
            <div>
              <label for="ev-enddate" class={labelCls}>End date</label>
              <input id="ev-enddate" type="date" bind:value={endDate} min={date} class="w-full {inputCls}" />
            </div>
          </div>
        {:else}
          <div class="grid grid-cols-3 gap-2">
            <div class="col-span-1">
              <label for="ev-date2" class={labelCls}>Date</label>
              <input id="ev-date2" type="date" bind:value={date} class="w-full {inputCls}" />
            </div>
            <div>
              <label for="ev-start" class={labelCls}>Start</label>
              <input id="ev-start" type="time" bind:value={startTime} class="w-full {inputCls}" />
            </div>
            <div>
              <label for="ev-end" class={labelCls}>End</label>
              <input id="ev-end" type="time" bind:value={endTime} class="w-full {inputCls}" />
            </div>
          </div>
          <!-- Timezone -->
          <div>
            <label for="ev-tz" class={labelCls}>Time zone</label>
            <select id="ev-tz" bind:value={timezone} class="w-full {inputCls}">
              {#each timezones as tz}
                <option value={tz}>{tz}</option>
              {/each}
            </select>
          </div>
        {/if}

        <!-- Location -->
        <div>
          <label for="ev-loc" class={labelCls}>Location</label>
          <input id="ev-loc" type="text" bind:value={location} placeholder="Room, address, or video link"
            class="w-full {inputCls}" />
        </div>

        <!-- Calendar + Status row -->
        <div class="grid grid-cols-2 gap-2">
          <div>
            <label for="ev-cal" class={labelCls}>Calendar</label>
            <select id="ev-cal" bind:value={calendarId} class="w-full {inputCls}">
              {#each $calendars as cal}
                <option value={cal.id}>{cal.name}</option>
              {/each}
            </select>
          </div>
          <div>
            <label for="ev-status" class={labelCls}>Status</label>
            <select id="ev-status" bind:value={status} class="w-full {inputCls}">
              <option value="confirmed">Confirmed</option>
              <option value="tentative">Tentative</option>
              <option value="cancelled">Cancelled</option>
            </select>
          </div>
        </div>

        <!-- Recurrence -->
        <div>
          <label for="ev-recur" class={labelCls}>Repeat</label>
          <select id="ev-recur" bind:value={recurrence} class="w-full {inputCls}">
            <option value="none">Does not repeat</option>
            <option value="daily">Daily</option>
            <option value="weekly">Weekly</option>
            <option value="monthly">Monthly</option>
            <option value="yearly">Yearly</option>
          </select>
        </div>

        <!-- Description -->
        <div>
          <label for="ev-desc" class={labelCls}>Description</label>
          <textarea id="ev-desc" bind:value={description} placeholder="Add notes or details…" rows="3"
            class="w-full {inputCls} resize-none"></textarea>
        </div>

      </div>

      <!-- Footer -->
      <div class="flex items-center justify-between px-5 py-3 border-t border-gray-200 dark:border-gray-700 flex-shrink-0">
        <div>
          {#if $editingEvent}
            <button on:click={remove} disabled={deleting}
              class="text-sm text-red-600 dark:text-red-400 hover:underline disabled:opacity-50">
              {deleting ? 'Deleting…' : 'Delete'}
            </button>
          {/if}
        </div>
        <div class="flex gap-2">
          <button on:click={close}
            class="px-4 py-1.5 text-sm rounded-lg border border-gray-200 dark:border-gray-600
                   text-gray-700 dark:text-gray-300 hover:bg-gray-50 dark:hover:bg-gray-700 transition-colors">
            Cancel
          </button>
          <button on:click={save} disabled={saving || !title.trim()}
            class="px-4 py-1.5 text-sm rounded-lg bg-blue-600 hover:bg-blue-700 text-white
                   disabled:opacity-50 transition-colors">
            {saving ? 'Saving…' : 'Save'}
          </button>
        </div>
      </div>

    </div>
  </div>
{/if}
