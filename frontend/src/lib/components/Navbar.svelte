<script>
  import { onMount } from 'svelte';
  import {
    appName, currentUser, darkMode, sieveOpen, appPasswordsOpen, shortcutsOpen,
    commandPaletteOpen, helpOpen
  } from '$lib/stores/mail.js';
  import { logout } from '$lib/api.js';
  import Avatar from './Avatar.svelte';
  import Spinner from './Spinner.svelte';
  import AppTabs from './AppTabs.svelte';
  import AppMenu from './AppMenu.svelte';
  import BrandMark from './BrandMark.svelte';
  import { isCompact, hasSidebar, toggleSidebar } from '$lib/stores/viewport.js';

  export let stalwartUrl = '';

  let open = false;
  let signingOut = false;
  let requestingNotif = false;

  function toggle() { open = !open; }
  function close()  { open = false; }

  function onKeydown(e) {
    if (open && e.key === 'Escape') close();
  }

  let notifPerm = 'unsupported';
  onMount(() => {
    if ('Notification' in window) notifPerm = Notification.permission;
  });

  async function requestNotifications() {
    if (!('Notification' in window) || requestingNotif) return;
    requestingNotif = true;
    try {
      await Notification.requestPermission();
      notifPerm = Notification.permission;
    } finally {
      requestingNotif = false;
    }
  }

  async function signOut() {
    if (signingOut) return;
    signingOut = true;
    await logout();
  }
</script>

<svelte:window on:keydown={onKeydown} />

<nav class="app-navbar flex items-center gap-3 px-3 flex-shrink-0 z-20
            bg-white dark:bg-gray-900 border-b border-gray-200 dark:border-gray-700">

  <!-- Drawer toggle. Only while the sidebar is an overlay *and* the current
       page actually has one. -->
  {#if $isCompact && $hasSidebar}
    <button
      on:click={toggleSidebar}
      aria-label="Toggle sidebar"
      class="w-9 h-9 -ml-1 flex items-center justify-center rounded-lg flex-shrink-0
             text-gray-600 dark:text-gray-300
             hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors duration-150"
    >
      <svg class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor"
           stroke-width="1.75" stroke-linecap="round">
        <path d="M3 6h18M3 12h18M3 18h18" />
      </svg>
    </button>
  {/if}

  <!-- Wordmark. Desktop only: on mobile the app menu already names where you
       are, and two labels competing for the same row is what made this cramped. -->
  <span class="hidden lg:flex items-center gap-1.5 flex-shrink-0 select-none
               text-sm font-semibold text-gray-800 dark:text-gray-100 tracking-tight">
    <BrandMark cls="w-4 h-4 opacity-80" />
    {$appName}
  </span>

  <!-- App switcher: tabs on desktop, a sheet menu on mobile -->
  <AppTabs />
  <AppMenu />

  <div class="flex-1"></div>

  <!-- Right controls -->
  <div class="flex items-center justify-end gap-1 flex-shrink-0">

  <!-- Command palette -->
  <button
    on:click={() => commandPaletteOpen.set(true)}
    title="Search and commands (Ctrl/Cmd + K)"
    class="hidden sm:flex items-center gap-2 h-7 pl-2 pr-1.5 mr-1 rounded-lg
           border border-gray-200 dark:border-gray-700
           text-gray-400 dark:text-gray-500
           hover:border-gray-300 dark:hover:border-gray-600
           hover:text-gray-600 dark:hover:text-gray-300
           transition-colors duration-150"
  >
    <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor"
         stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <circle cx="11" cy="11" r="8" /><path d="m21 21-4.35-4.35" />
    </svg>
    <kbd class="text-[10px] font-sans px-1 py-0.5 rounded
                bg-gray-100 dark:bg-gray-800 text-gray-400 dark:text-gray-500">⌘K</kbd>
  </button>

  <!-- Notification bell -->
  {#if notifPerm !== 'unsupported' && notifPerm !== 'denied'}
    <button
      on:click={requestNotifications}
      disabled={requestingNotif}
      title={notifPerm === 'granted' ? 'Notifications enabled' : 'Enable notifications'}
      class="hidden sm:flex w-8 h-8 items-center justify-center rounded-lg transition-colors duration-150
             disabled:opacity-50
             {notifPerm === 'granted'
               ? 'text-blue-500 dark:text-blue-400'
               : 'text-gray-500 dark:text-gray-400 hover:bg-gray-100 dark:hover:bg-gray-800'}"
    >
      {#if requestingNotif}
        <Spinner size="xs" label="Requesting permission" />
      {:else if notifPerm === 'granted'}
        <svg class="w-4 h-4" viewBox="0 0 24 24" fill="currentColor">
          <path d="M12 22c1.1 0 2-.9 2-2h-4c0 1.1.9 2 2 2zm6-6v-5c0-3.07-1.63-5.64-4.5-6.32V4c0-.83-.67-1.5-1.5-1.5s-1.5.67-1.5 1.5v.68C7.64 5.36 6 7.92 6 11v5l-2 2v1h16v-1l-2-2z"/>
        </svg>
      {:else}
        <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
          <path d="M18 8A6 6 0 006 8c0 7-3 9-3 9h18s-3-2-3-9"/><path d="M13.73 21a2 2 0 01-3.46 0"/>
        </svg>
      {/if}
    </button>
  {/if}

  <!-- Documentation -->
  <button
    on:click={() => helpOpen.set(true)}
    title="Help and documentation"
    aria-label="Help and documentation"
    class="w-8 h-8 flex items-center justify-center rounded-lg text-gray-500 dark:text-gray-400
           hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors duration-150"
  >
    <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75"
         stroke-linecap="round" stroke-linejoin="round">
      <circle cx="12" cy="12" r="9" />
      <path d="M9.6 9.2a2.5 2.5 0 1 1 3.2 3.1c-.5.2-.8.7-.8 1.2v.4" />
      <path d="M12 17h.01" />
    </svg>
  </button>

  <!-- Keyboard shortcuts -->
  <button
    on:click={() => shortcutsOpen.set(true)}
    title="Keyboard shortcuts ( ? )"
    class="hidden md:flex w-8 h-8 items-center justify-center rounded-lg text-gray-500 dark:text-gray-400
           hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors duration-150"
  >
    <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75"
         stroke-linecap="round" stroke-linejoin="round">
      <rect x="2" y="6" width="20" height="12" rx="2" />
      <path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M8 14h8" />
    </svg>
  </button>

  <!-- Dark mode toggle -->
  <button
    on:click={() => darkMode.toggle()}
    title="Toggle dark mode"
    class="w-8 h-8 flex items-center justify-center rounded-lg text-gray-500 dark:text-gray-400
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
      class="p-1 rounded-lg hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors duration-150"
      title={$currentUser || 'Account'}
    >
      <Avatar name={$currentUser} email={$currentUser} size="sm" />
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
            on:click={() => { helpOpen.set(true); close(); }}
            class="flex items-center gap-3 w-full px-4 py-2.5 text-sm text-gray-700 dark:text-gray-300
                   hover:bg-gray-50 dark:hover:bg-gray-700/60 transition-colors duration-100"
          >
            <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75"
                 stroke-linecap="round" stroke-linejoin="round">
              <circle cx="12" cy="12" r="9" />
              <path d="M9.6 9.2a2.5 2.5 0 1 1 3.2 3.1c-.5.2-.8.7-.8 1.2v.4" />
              <path d="M12 17h.01" />
            </svg>
            Help
          </button>

          <button
            on:click={() => { shortcutsOpen.set(true); close(); }}
            class="md:hidden flex items-center gap-3 w-full px-4 py-2.5 text-sm text-gray-700 dark:text-gray-300
                   hover:bg-gray-50 dark:hover:bg-gray-700/60 transition-colors duration-100"
          >
            <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75"
                 stroke-linecap="round" stroke-linejoin="round">
              <rect x="2" y="6" width="20" height="12" rx="2" />
              <path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M8 14h8" />
            </svg>
            Keyboard shortcuts
          </button>

          <button
            on:click={() => { commandPaletteOpen.set(true); close(); }}
            class="sm:hidden flex items-center gap-3 w-full px-4 py-2.5 text-sm text-gray-700 dark:text-gray-300
                   hover:bg-gray-50 dark:hover:bg-gray-700/60 transition-colors duration-100"
          >
            <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                 stroke-linecap="round" stroke-linejoin="round">
              <circle cx="11" cy="11" r="8" /><path d="m21 21-4.35-4.35" />
            </svg>
            Search and commands
          </button>

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

          <button
            on:click={() => { appPasswordsOpen.set(true); close(); }}
            class="flex items-center gap-3 w-full px-4 py-2 text-sm text-gray-700 dark:text-gray-300
                   hover:bg-gray-50 dark:hover:bg-gray-700/60 transition-colors duration-100"
          >
            <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                 stroke-linecap="round" stroke-linejoin="round">
              <rect x="3" y="11" width="18" height="11" rx="2" ry="2"/>
              <path d="M7 11V7a5 5 0 0110 0v4"/>
            </svg>
            App Passwords
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
            on:click={signOut}
            disabled={signingOut}
            class="flex items-center gap-3 w-full px-4 py-2 text-sm text-red-600 dark:text-red-400
                   hover:bg-red-50 dark:hover:bg-red-900/20 disabled:opacity-60
                   transition-colors duration-100"
          >
            {#if signingOut}
              <Spinner size="xs" label="" accent="border-t-red-500" />
            {:else}
              <!-- sign out icon -->
              <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none"
                   stroke="currentColor" stroke-width="2">
                <path d="M9 21H5a2 2 0 01-2-2V5a2 2 0 012-2h4"/>
                <polyline points="16 17 21 12 16 7"/>
                <line x1="21" y1="12" x2="9" y2="12"/>
              </svg>
            {/if}
            {signingOut ? 'Signing out…' : 'Sign out'}
          </button>
        </div>
      </div>
    {/if}
  </div>
  </div>
</nav>
