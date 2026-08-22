<script>
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';

  import { APPS, activeApp } from '$lib/apps.js';

  $: active = activeApp($page.url.pathname);
</script>

<nav class="flex items-center gap-0.5" aria-label="Applications">
  {#each APPS as app (app.id)}
    {@const isActive = active === app.id}
    <button
      on:click={() => goto(app.href)}
      aria-current={isActive ? 'page' : undefined}
      title={app.label}
      class="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-xs font-medium
             transition-colors duration-150
             {isActive
               ? 'text-blue-600 dark:text-blue-400 bg-blue-50 dark:bg-blue-900/40'
               : 'text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-200 hover:bg-gray-100 dark:hover:bg-gray-800'}"
    >
      <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor"
           stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
        {#if app.id === 'mail'}
          <path d="M21.75 6.75v10.5a2.25 2.25 0 01-2.25 2.25h-15a2.25 2.25 0 01-2.25-2.25V6.75m19.5 0A2.25 2.25 0 0019.5 4.5h-15a2.25 2.25 0 00-2.25 2.25m19.5 0v.243a2.25 2.25 0 01-1.07 1.916l-7.5 4.615a2.25 2.25 0 01-2.36 0L3.32 8.91a2.25 2.25 0 01-1.07-1.916V6.75" />
        {:else if app.id === 'calendar'}
          <rect x="3" y="4" width="18" height="18" rx="2" /><path d="M16 2v4M8 2v4M3 10h18" />
        {:else if app.id === 'contacts'}
          <path d="M17 21v-2a4 4 0 00-4-4H5a4 4 0 00-4 4v2" /><circle cx="9" cy="7" r="4" />
          <path d="M23 21v-2a4 4 0 00-3-3.87M16 3.13a4 4 0 010 7.75" />
        {:else if app.id === 'notes'}
          <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
          <path d="M14 3v5h5M9 13h6M9 17h4" />
        {:else}
          <path d="M3 7a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
        {/if}
      </svg>
      <!-- Labels are the first thing to go when the window narrows; the icons
           and tooltips carry the meaning below that point. -->
      <span class="hidden lg:inline">{app.label}</span>
    </button>
  {/each}
</nav>
