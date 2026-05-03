<script>
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';

  $: activeView = $page.url.pathname === '/calendar' ? 'calendar'
                : $page.url.pathname === '/contacts'  ? 'contacts'
                : 'mail';
</script>

<div class="flex-shrink-0 flex items-center justify-around px-3 py-2
            border-t border-gray-200 dark:border-gray-700">

  <button
    on:click={() => goto('/')}
    title="Mail"
    class="flex flex-col items-center gap-0.5 px-3 py-1.5 rounded-lg text-xs transition-colors duration-150
           {activeView === 'mail'
             ? 'text-blue-600 dark:text-blue-400 bg-blue-50 dark:bg-blue-900/30'
             : 'text-gray-500 dark:text-gray-400 hover:bg-gray-200 dark:hover:bg-gray-800'}"
  >
    <svg class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
      <path d="M21.75 6.75v10.5a2.25 2.25 0 01-2.25 2.25h-15a2.25 2.25 0 01-2.25-2.25V6.75m19.5 0A2.25 2.25 0 0019.5 4.5h-15a2.25 2.25 0 00-2.25 2.25m19.5 0v.243a2.25 2.25 0 01-1.07 1.916l-7.5 4.615a2.25 2.25 0 01-2.36 0L3.32 8.91a2.25 2.25 0 01-1.07-1.916V6.75"/>
    </svg>
    Mail
  </button>

  <button
    on:click={() => goto('/calendar')}
    title="Calendar"
    class="flex flex-col items-center gap-0.5 px-3 py-1.5 rounded-lg text-xs transition-colors duration-150
           {activeView === 'calendar'
             ? 'text-blue-600 dark:text-blue-400 bg-blue-50 dark:bg-blue-900/30'
             : 'text-gray-500 dark:text-gray-400 hover:bg-gray-200 dark:hover:bg-gray-800'}"
  >
    <svg class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
      <rect x="3" y="4" width="18" height="18" rx="2"/><path d="M16 2v4M8 2v4M3 10h18"/>
    </svg>
    Calendar
  </button>

  <button
    on:click={() => goto('/contacts')}
    title="Contacts"
    class="flex flex-col items-center gap-0.5 px-3 py-1.5 rounded-lg text-xs transition-colors duration-150
           {activeView === 'contacts'
             ? 'text-blue-600 dark:text-blue-400 bg-blue-50 dark:bg-blue-900/30'
             : 'text-gray-500 dark:text-gray-400 hover:bg-gray-200 dark:hover:bg-gray-800'}"
  >
    <svg class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
      <path d="M17 21v-2a4 4 0 00-4-4H5a4 4 0 00-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M23 21v-2a4 4 0 00-3-3.87M16 3.13a4 4 0 010 7.75"/>
    </svg>
    Contacts
  </button>

</div>
