<script>
  import { fly, fade } from 'svelte/transition';
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { APPS, activeApp, appFor } from '$lib/apps.js';
  import AppIcon from './AppIcon.svelte';
  import { inboxUnread } from '$lib/stores/mail.js';

  let open = false;

  $: active = activeApp($page.url.pathname);
  $: current = appFor($page.url.pathname);

  function choose(app) {
    open = false;
    if (app.id !== active) goto(app.href);
  }

  function onKeydown(event) {
    if (open && event.key === 'Escape') open = false;
  }
</script>

<svelte:window on:keydown={onKeydown} />

<!--
  The mobile counterpart to AppTabs. Six icon tabs plus a wordmark and the
  right-hand controls do not fit in a phone navbar, and shrinking them to
  unlabelled icons trades one problem for a worse one. A single control naming
  the current app is both the label and the switcher.
-->
<button
  on:click={() => (open = !open)}
  aria-haspopup="menu"
  aria-expanded={open}
  class="lg:hidden flex items-center gap-1.5 px-2 py-1.5 rounded-lg min-w-0
         text-sm font-semibold text-gray-800 dark:text-gray-100
         hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors duration-150"
>
  <AppIcon id={current.id} cls="w-4 h-4 flex-shrink-0 text-blue-500 dark:text-blue-400" />
  <span class="truncate">{current.label}</span>
  <svg class="w-3 h-3 flex-shrink-0 text-gray-400 transition-transform duration-150"
       style="transform: rotate({open ? '180deg' : '0deg'})"
       viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"
       stroke-linecap="round" stroke-linejoin="round">
    <path d="m6 9 6 6 6-6" />
  </svg>
</button>

{#if open}
  <div
    class="lg:hidden fixed inset-0 top-[var(--navbar-height)] z-[55]
           bg-gray-900/40 dark:bg-black/60"
    transition:fade={{ duration: 120 }}
    on:click={() => (open = false)}
    role="presentation"
  ></div>

  <!-- A sheet rather than a dropdown: at this width a dropdown would be nearly
       full-bleed anyway, and a sheet puts the targets within thumb reach. -->
  <div
    class="lg:hidden fixed inset-x-0 bottom-0 z-[56] pb-safe
           rounded-t-2xl bg-white dark:bg-gray-800
           shadow-2xl ring-1 ring-gray-900/5 dark:ring-white/10"
    transition:fly={{ y: 240, duration: 200 }}
    role="menu"
    aria-label="Applications"
  >
    <div class="flex justify-center pt-2 pb-1">
      <div class="w-9 h-1 rounded-full bg-gray-300 dark:bg-gray-600"></div>
    </div>

    <div class="grid grid-cols-3 gap-1 p-3">
      {#each APPS as app (app.id)}
        {@const isActive = active === app.id}
        <button
          on:click={() => choose(app)}
          role="menuitem"
          aria-current={isActive ? 'page' : undefined}
          class="relative flex flex-col items-center justify-center gap-1.5 py-3.5 rounded-xl
                 transition-colors duration-150
                 {isActive
                   ? 'bg-blue-50 dark:bg-blue-900/40 text-blue-600 dark:text-blue-400'
                   : 'text-gray-600 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-700'}"
        >
          <AppIcon id={app.id} cls="w-6 h-6" />
          <span class="text-xs font-medium">{app.label}</span>

          {#if app.id === 'mail' && $inboxUnread > 0}
            <span class="absolute top-2 right-1/2 translate-x-5 min-w-[1.1rem] h-[1.1rem] px-1
                         flex items-center justify-center rounded-full
                         text-[10px] font-semibold bg-blue-500 text-white">
              {$inboxUnread > 99 ? '99+' : $inboxUnread}
            </span>
          {/if}
        </button>
      {/each}
    </div>
  </div>
{/if}
