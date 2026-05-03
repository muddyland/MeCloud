<script>
  import { appName, currentUser, darkMode, sieveOpen } from '$lib/stores/mail.js';
  import { logout } from '$lib/api.js';
  import Avatar from './Avatar.svelte';

  export let stalwartUrl = '';

  let open = false;

  function toggle() { open = !open; }
  function close()  { open = false; }
</script>

<nav class="h-11 grid grid-cols-3 items-center px-4 flex-shrink-0 z-20
            bg-white dark:bg-gray-900 border-b border-gray-200 dark:border-gray-700">

  <!-- Left spacer (keeps center column truly centered) -->
  <div></div>

  <!-- App name — center -->
  <span class="text-sm font-semibold text-gray-800 dark:text-gray-100 tracking-tight select-none
               flex items-center justify-center gap-1.5">
    <svg class="w-4 h-4 opacity-70" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
      <path d="M21.75 6.75v10.5a2.25 2.25 0 01-2.25 2.25h-15a2.25 2.25 0 01-2.25-2.25V6.75m19.5 0A2.25 2.25 0 0019.5 4.5h-15a2.25 2.25 0 00-2.25 2.25m19.5 0v.243a2.25 2.25 0 01-1.07 1.916l-7.5 4.615a2.25 2.25 0 01-2.36 0L3.32 8.91a2.25 2.25 0 01-1.07-1.916V6.75"/>
    </svg>
    {$appName}
  </span>

  <!-- Right controls -->
  <div class="flex items-center justify-end gap-1">
  <!-- Dark mode toggle -->
  <button
    on:click={() => darkMode.toggle()}
    title="Toggle dark mode"
    class="p-1.5 rounded-lg text-gray-500 dark:text-gray-400
           hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors duration-150"
  >
    {#if $darkMode}
      <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
        <circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"/>
      </svg>
    {:else}
      <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
        <path d="M21 12.79A9 9 0 1111.21 3 7 7 0 0021 12.79z"/>
      </svg>
    {/if}
  </button>

  <!-- User menu -->
  <div class="relative">
    <button
      on:click={toggle}
      class="flex items-center gap-2 px-2 py-1 rounded-lg
             hover:bg-gray-100 dark:hover:bg-gray-800
             transition-colors duration-150"
    >
      <Avatar name={$currentUser} email={$currentUser} size="sm" />
      <span class="text-sm text-gray-700 dark:text-gray-300 max-w-[160px] truncate">
        {$currentUser || '…'}
      </span>
      <!-- chevron -->
      <svg class="w-3.5 h-3.5 text-gray-400 transition-transform duration-150
                  {open ? 'rotate-180' : ''}"
           viewBox="0 0 20 20" fill="currentColor">
        <path fill-rule="evenodd"
              d="M5.23 7.21a.75.75 0 011.06.02L10 11.168l3.71-3.938a.75.75 0 111.08 1.04l-4.25 4.5a.75.75 0 01-1.08 0l-4.25-4.5a.75.75 0 01.02-1.06z"
              clip-rule="evenodd" />
      </svg>
    </button>

    {#if open}
      <!-- Backdrop to close on outside click -->
      <div class="fixed inset-0 z-10" on:click={close} aria-hidden="true"></div>

      <!-- Dropdown -->
      <div class="absolute right-0 mt-1 w-56 rounded-xl shadow-lg ring-1
                  bg-white dark:bg-gray-800
                  ring-black/5 dark:ring-white/10 z-20 overflow-hidden
                  animate-in fade-in slide-in-from-top-1 duration-100">

        <!-- User info header -->
        <div class="px-4 py-3 border-b border-gray-100 dark:border-gray-700">
          <p class="text-xs text-gray-500 dark:text-gray-400">Signed in as</p>
          <p class="text-sm font-medium text-gray-800 dark:text-gray-100 truncate mt-0.5">
            {$currentUser || '—'}
          </p>
        </div>

        <div class="py-1">
          <button
            on:click={() => { sieveOpen.set(true); close(); }}
            class="flex items-center gap-3 w-full px-4 py-2 text-sm text-gray-700 dark:text-gray-300
                   hover:bg-gray-50 dark:hover:bg-gray-700/60 transition-colors duration-100"
          >
            <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <polygon points="22 3 2 3 10 12.46 10 19 14 21 14 12.46 22 3"/>
            </svg>
            Filters / Rules
          </button>

          {#if stalwartUrl}
            <a
              href="{stalwartUrl}/admin"
              target="_blank"
              rel="noopener noreferrer"
              on:click={close}
              class="flex items-center gap-3 w-full px-4 py-2 text-sm text-gray-700 dark:text-gray-300
                     hover:bg-gray-50 dark:hover:bg-gray-700/60 transition-colors duration-100"
            >
              Stalwart Admin
              <svg class="w-3.5 h-3.5 ml-auto text-gray-400" viewBox="0 0 24 24" fill="none"
                   stroke="currentColor" stroke-width="2">
                <path d="M18 13v6a2 2 0 01-2 2H5a2 2 0 01-2-2V8a2 2 0 012-2h6"/>
                <polyline points="15 3 21 3 21 9"/>
                <line x1="10" y1="14" x2="21" y2="3"/>
              </svg>
            </a>
          {/if}

          <button
            on:click={logout}
            class="flex items-center gap-3 w-full px-4 py-2 text-sm text-red-600 dark:text-red-400
                   hover:bg-red-50 dark:hover:bg-red-900/20 transition-colors duration-100"
          >
            <!-- sign out icon -->
            <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none"
                 stroke="currentColor" stroke-width="2">
              <path d="M9 21H5a2 2 0 01-2-2V5a2 2 0 012-2h4"/>
              <polyline points="16 17 21 12 16 7"/>
              <line x1="21" y1="12" x2="9" y2="12"/>
            </svg>
            Sign out
          </button>
        </div>
      </div>
    {/if}
  </div>
  </div>
</nav>
