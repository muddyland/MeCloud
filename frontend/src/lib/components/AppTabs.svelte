<script>
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { APPS, activeApp } from '$lib/apps.js';
  import AppIcon from './AppIcon.svelte';

  $: active = activeApp($page.url.pathname);
</script>

<!--
  Desktop only. Six destinations do not fit alongside a wordmark and the
  right-hand controls on a phone, so below `lg` the AppMenu replaces this with a
  single control that opens a sheet.
-->
<nav class="hidden lg:flex items-center gap-0.5" aria-label="Applications">
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
      <AppIcon id={app.id} cls="w-4 h-4 flex-shrink-0" />
      <span class="hidden xl:inline">{app.label}</span>
    </button>
  {/each}
</nav>
