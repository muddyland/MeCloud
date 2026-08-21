<script>
  import { onMount } from 'svelte';
  import Navbar from '$lib/components/Navbar.svelte';
  import CalendarGrid from '$lib/components/CalendarGrid.svelte';
  import EventModal from '$lib/components/EventModal.svelte';
  import Toasts from '$lib/components/Toasts.svelte';
  import Spinner from '$lib/components/Spinner.svelte';
  import {
    calendars, calendarEvents, calendarLoading, selectedCalendar, calendarDate, calendarRefreshToken
  } from '$lib/stores/calendar.js';
  import { jmapAccountId, jmapSession, currentUser, sidebarWidth } from '$lib/stores/mail.js';
  import {
    getCalendars, getCalendarEvents, getAppConfig, getJMAPSession,
    createCalendar, deleteCalendar
  } from '$lib/api.js';
  import { toast } from '$lib/stores/toast.js';

  let stalwartUrl    = '';
  let newCalInput    = false;
  let newCalName     = '';
  let creatingCal    = false;
  let deletingCalId  = null;

  async function loadEvents() {
    if (!$jmapAccountId) return;
    calendarLoading.set(true);
    try {
      const d      = $calendarDate;
      const after  = new Date(d.getFullYear(), d.getMonth(), 1).toISOString();
      const before = new Date(d.getFullYear(), d.getMonth() + 1, 1).toISOString();
      const events = await getCalendarEvents($jmapAccountId, $jmapSession, $selectedCalendar, after, before);
      calendarEvents.set(events);
    } catch (e) {
      console.error('Failed to load calendar events', e);
    } finally {
      calendarLoading.set(false);
    }
  }

  let prevKey = null;
  $: {
    const key = `${$calendarDate?.getFullYear()}-${$calendarDate?.getMonth()}-${$selectedCalendar}-${$calendarRefreshToken}`;
    if (key !== prevKey && $jmapAccountId) { prevKey = key; loadEvents(); }
  }

  // ── Calendar management ──────────────────────────────────────────────────────
  async function confirmNewCal() {
    if (!newCalName.trim()) { newCalInput = false; return; }
    creatingCal = true;
    try {
      const created = await createCalendar($jmapAccountId, $jmapSession, newCalName.trim());
      if (created) {
        calendars.update(list => [...list, { id: created.id, name: newCalName.trim() }]);
        toast('Calendar created', 'success');
      }
    } catch (e) { toast(e?.message ?? 'Create failed', 'error'); }
    finally { creatingCal = false; newCalInput = false; newCalName = ''; }
  }

  async function confirmDeleteCal(cal) {
    deletingCalId = cal.id;
    try {
      await deleteCalendar($jmapAccountId, $jmapSession, cal.id);
      calendars.update(list => list.filter(c => c.id !== cal.id));
      if ($selectedCalendar === cal.id) selectedCalendar.set(null);
      toast('Calendar deleted', 'success');
    } catch (e) { toast(e?.message ?? 'Delete failed', 'error'); }
    finally { deletingCalId = null; }
  }

  onMount(async () => {
    const config = await getAppConfig();
    stalwartUrl = config.stalwartUrl ?? '';

    if (!$jmapAccountId) {
      try {
        const session = await getJMAPSession();
        jmapSession.set(session);
        const accountId = Object.keys(session.accounts)[0];
        jmapAccountId.set(accountId);
        currentUser.set(session.username || session.accounts[accountId]?.name || '');
      } catch (e) { console.error('JMAP session failed', e); }
    }

    if ($jmapAccountId) {
      try {
        const cals = await getCalendars($jmapAccountId, $jmapSession);
        calendars.set(cals);
      } catch (e) { console.error('Failed to load calendars', e); }
      await loadEvents();
    }
  });
</script>

<div class="flex flex-col h-screen bg-gray-100 dark:bg-gray-950">
  <Navbar {stalwartUrl} />

  <div class="flex flex-1 min-h-0 overflow-hidden">

    <!-- Left panel -->
    <div class="flex-shrink-0 h-full flex flex-col bg-gray-100 dark:bg-gray-900
                border-r border-gray-200 dark:border-gray-700" style="width: {$sidebarWidth}px">

      <div class="flex-1 overflow-y-auto px-2 py-2">

        <!-- Section heading + new calendar button -->
        <div class="px-1 pt-2 pb-1 flex items-center justify-between">
          <span class="text-xs font-semibold text-gray-400 dark:text-gray-500 uppercase tracking-wider select-none">
            Calendars
          </span>
          <button on:click={() => { newCalInput = true; newCalName = ''; }}
            title="New calendar"
            class="w-5 h-5 flex items-center justify-center rounded text-gray-400 hover:text-gray-600
                   dark:hover:text-gray-300 hover:bg-gray-200 dark:hover:bg-gray-700 transition-colors text-base leading-none">
            +
          </button>
        </div>

        {#if newCalInput}
          <div class="flex items-center gap-1 px-2 py-1">
            <input bind:value={newCalName}
              on:keydown={(e) => { if (e.key === 'Enter') confirmNewCal(); if (e.key === 'Escape') newCalInput = false; }}
              placeholder="Calendar name"
              class="flex-1 text-xs px-2 py-1 rounded border border-blue-400 dark:border-blue-500
                     bg-white dark:bg-gray-700 text-gray-800 dark:text-gray-100 focus:outline-none"
              autofocus />
            <button on:click={confirmNewCal} disabled={creatingCal} title="Create"
              class="text-xs text-blue-600 dark:text-blue-400 disabled:opacity-50
                     w-4 flex items-center justify-center">
              {#if creatingCal}<Spinner size="xs" label="Creating" />{:else}✓{/if}
            </button>
            <button on:click={() => newCalInput = false} class="text-xs text-gray-400">✕</button>
          </div>
        {/if}

        <!-- All Calendars -->
        <button on:click={() => selectedCalendar.set(null)}
          class="w-full flex items-center gap-2 px-3 py-2 rounded-lg text-sm transition-colors duration-150
                 {$selectedCalendar === null
                   ? 'bg-blue-100 dark:bg-blue-900/50 text-blue-700 dark:text-blue-300 font-medium'
                   : 'text-gray-700 dark:text-gray-300 hover:bg-gray-200 dark:hover:bg-gray-800'}">
          <span class="w-2.5 h-2.5 rounded-full bg-blue-500 flex-shrink-0"></span>
          All Calendars
        </button>

        {#each $calendars as cal}
          <div class="group relative">
            <button on:click={() => selectedCalendar.set(cal.id)}
              class="w-full flex items-center gap-2 px-3 py-2 rounded-lg text-sm transition-colors duration-150
                     {$selectedCalendar === cal.id
                       ? 'bg-blue-100 dark:bg-blue-900/50 text-blue-700 dark:text-blue-300 font-medium'
                       : 'text-gray-700 dark:text-gray-300 hover:bg-gray-200 dark:hover:bg-gray-800'}">
              <span class="w-2.5 h-2.5 rounded-full bg-blue-400 flex-shrink-0"></span>
              <span class="truncate flex-1 text-left">{cal.name}</span>
            </button>
            <button on:click={() => confirmDeleteCal(cal)} disabled={deletingCalId === cal.id}
              title="Delete calendar"
              class="absolute right-1 inset-y-0 flex items-center px-1 transition-colors
                     text-gray-400 hover:text-red-500 dark:hover:text-red-400
                     {deletingCalId === cal.id ? '' : 'hidden group-hover:flex'}">
              {#if deletingCalId === cal.id}
                <Spinner size="xs" label="Deleting calendar" accent="border-t-red-500" />
              {:else}
                <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
                  <path d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 01-2.244 2.077H8.084a2.25 2.25 0 01-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 00-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 013.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 00-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 00-7.5 0"/>
                </svg>
              {/if}
            </button>
          </div>
        {/each}
      </div>

    </div>

    <!-- Calendar grid -->
    <div class="flex-1 min-w-0 h-full bg-white dark:bg-gray-900">
      {#if $calendarLoading}
        <div class="flex flex-col items-center justify-center h-full gap-3">
          <Spinner size="lg" label="" />
          <p class="text-sm text-gray-400 dark:text-gray-500">Loading events…</p>
        </div>
      {:else}
        <CalendarGrid />
      {/if}
    </div>

  </div>
</div>

<EventModal />
<Toasts />
