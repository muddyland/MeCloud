<script>
  import { calendarEvents, calendarDate, eventModalOpen, editingEvent, newEventDate } from '$lib/stores/calendar.js';

  const DAYS = ['Sun','Mon','Tue','Wed','Thu','Fri','Sat'];

  $: year  = $calendarDate.getFullYear();
  $: month = $calendarDate.getMonth();

  $: monthLabel = $calendarDate.toLocaleString('default', { month: 'long', year: 'numeric' });

  $: weeks = buildWeeks(year, month);

  // Reactive map: dateString → events[], rebuilt whenever the store changes
  $: eventsByDate = $calendarEvents.reduce((acc, e) => {
    const key = e.start?.slice(0, 10) ?? '';
    if (!key) return acc;
    if (!acc[key]) acc[key] = [];
    acc[key].push(e);
    return acc;
  }, {});

  function buildWeeks(y, m) {
    const first = new Date(y, m, 1);
    const last  = new Date(y, m + 1, 0);
    const cells = [];

    // pad start
    for (let i = 0; i < first.getDay(); i++) {
      const d = new Date(y, m, 1 - (first.getDay() - i));
      cells.push({ date: d, inMonth: false });
    }
    for (let d = 1; d <= last.getDate(); d++) {
      cells.push({ date: new Date(y, m, d), inMonth: true });
    }
    // pad end to full weeks
    while (cells.length % 7 !== 0) {
      const d = new Date(y, m + 1, cells.length - last.getDate() - first.getDay() + 1);
      cells.push({ date: d, inMonth: false });
    }

    const rows = [];
    for (let i = 0; i < cells.length; i += 7) rows.push(cells.slice(i, i + 7));
    return rows;
  }

  function eventsForDate(date) {
    return eventsByDate[fmtDate(date)] ?? [];
  }

  function fmtDate(d) {
    return `${d.getFullYear()}-${String(d.getMonth()+1).padStart(2,'0')}-${String(d.getDate()).padStart(2,'0')}`;
  }

  function isToday(d) {
    const t = new Date();
    return d.getFullYear() === t.getFullYear() && d.getMonth() === t.getMonth() && d.getDate() === t.getDate();
  }

  function prevMonth() {
    calendarDate.update(d => new Date(d.getFullYear(), d.getMonth() - 1, 1));
  }
  function nextMonth() {
    calendarDate.update(d => new Date(d.getFullYear(), d.getMonth() + 1, 1));
  }
  function goToday() {
    calendarDate.set(new Date());
  }

  function openNewEvent(cell) {
    newEventDate.set(cell.date);
    editingEvent.set(null);
    eventModalOpen.set(true);
  }

  function openEvent(e, ev) {
    ev.stopPropagation();
    editingEvent.set(e);
    newEventDate.set(null);
    eventModalOpen.set(true);
  }

  function fmtTime(start) {
    if (!start?.includes('T')) return '';
    const t = start.split('T')[1].slice(0, 5);
    const [h, m] = t.split(':').map(Number);
    const ampm = h >= 12 ? 'pm' : 'am';
    return `${h % 12 || 12}:${String(m).padStart(2,'0')}${ampm}`;
  }
</script>

<div class="flex flex-col h-full">

  <!-- Month nav -->
  <div class="flex items-center gap-3 px-4 py-3 flex-shrink-0 border-b border-gray-200 dark:border-gray-700">
    <button on:click={prevMonth}
      class="p-1.5 rounded-lg text-gray-500 hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors">
      <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M15 18l-6-6 6-6"/></svg>
    </button>
    <button on:click={nextMonth}
      class="p-1.5 rounded-lg text-gray-500 hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors">
      <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M9 18l6-6-6-6"/></svg>
    </button>
    <span class="text-sm font-semibold text-gray-800 dark:text-gray-100">{monthLabel}</span>
    <button on:click={goToday}
      class="ml-auto text-xs px-3 py-1 rounded-lg border border-gray-200 dark:border-gray-600
             text-gray-600 dark:text-gray-400 hover:bg-gray-50 dark:hover:bg-gray-800 transition-colors">
      Today
    </button>
    <button
      on:click={() => { editingEvent.set(null); newEventDate.set(new Date()); eventModalOpen.set(true); }}
      class="text-xs px-3 py-1 rounded-lg bg-blue-600 hover:bg-blue-700 text-white transition-colors"
    >+ Event</button>
  </div>

  <!-- Day-of-week headers -->
  <div class="grid grid-cols-7 flex-shrink-0 border-b border-gray-200 dark:border-gray-700">
    {#each DAYS as day}
      <div class="py-2 text-center text-xs font-medium text-gray-500 dark:text-gray-400">{day}</div>
    {/each}
  </div>

  <!-- Calendar grid -->
  <div class="flex-1 overflow-y-auto">
    {#each weeks as week}
      <div class="grid grid-cols-7 border-b border-gray-200 dark:border-gray-700 last:border-b-0" style="min-height: 100px">
        {#each week as cell}
          <!-- svelte-ignore a11y-click-events-have-key-events -->
          <!-- svelte-ignore a11y-no-static-element-interactions -->
          <div
            on:click={() => openNewEvent(cell)}
            class="border-r border-gray-200 dark:border-gray-700 last:border-r-0 p-1 cursor-pointer
                   hover:bg-gray-50 dark:hover:bg-gray-800/50 transition-colors"
          >
            <div class="flex items-center justify-center w-6 h-6 mb-0.5 mx-auto rounded-full text-xs select-none
                        {isToday(cell.date)
                          ? 'bg-blue-600 text-white font-semibold'
                          : cell.inMonth
                            ? 'text-gray-800 dark:text-gray-200'
                            : 'text-gray-300 dark:text-gray-600'}">
              {cell.date.getDate()}
            </div>

            <!-- Events for this day -->
            {#each eventsForDate(cell.date).slice(0, 3) as ev}
              <!-- svelte-ignore a11y-click-events-have-key-events -->
              <div
                on:click={(e) => openEvent(ev, e)}
                class="text-xs px-1.5 py-0.5 mb-0.5 rounded bg-blue-100 dark:bg-blue-900/50
                       text-blue-800 dark:text-blue-200 truncate cursor-pointer
                       hover:bg-blue-200 dark:hover:bg-blue-800/60"
                title={ev.title}
              >
                {#if fmtTime(ev.start)}<span class="opacity-60 mr-1">{fmtTime(ev.start)}</span>{/if}{ev.title}
              </div>
            {/each}
            {#if eventsForDate(cell.date).length > 3}
              <div class="text-xs text-gray-400 dark:text-gray-500 px-1">
                +{eventsForDate(cell.date).length - 3} more
              </div>
            {/if}
          </div>
        {/each}
      </div>
    {/each}
  </div>

</div>
