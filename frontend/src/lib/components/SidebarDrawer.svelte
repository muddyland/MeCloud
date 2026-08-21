<script>
  import { fade } from 'svelte/transition';
  import { isCompact, sidebarOpen, closeSidebar } from '$lib/stores/viewport.js';

  /** Width in px at desktop widths. Ignored while the drawer is an overlay. */
  export let width = 240;
  /** Extra classes for the panel itself. */
  export let cls = '';

  function onKeydown(event) {
    if (event.key === 'Escape' && $isCompact && $sidebarOpen) closeSidebar();
  }
</script>

<svelte:window on:keydown={onKeydown} />

{#if $isCompact && $sidebarOpen}
  <!-- Backdrop exists only while the drawer overlays, so it can never swallow
       clicks at desktop widths. -->
  <div
    class="fixed inset-0 top-11 z-30 bg-gray-900/40 dark:bg-black/60 lg:hidden"
    transition:fade={{ duration: 150 }}
    on:click={closeSidebar}
    role="presentation"
  ></div>
{/if}

<!--
  The compact/desktop split is expressed entirely in CSS. The static build is
  prerendered with no media-query knowledge, so a JS-driven version renders the
  desktop layout for a frame before hydration — a visible lurch on a phone.
  Only `sidebarOpen` is JS, and it defaults closed, which is already correct.
-->
<aside
  style="--sidebar-width: {width}px"
  class="flex flex-col bg-gray-100 dark:bg-gray-900
         border-r border-gray-200 dark:border-gray-700 {cls}
         fixed top-11 bottom-0 left-0 z-40 w-[85vw] max-w-xs shadow-2xl
         transition-transform duration-200 ease-out
         {$sidebarOpen ? 'translate-x-0' : '-translate-x-full'}
         lg:relative lg:top-auto lg:bottom-auto lg:z-auto
         lg:h-full lg:w-[var(--sidebar-width)] lg:max-w-none
         lg:shadow-none lg:translate-x-0 lg:flex-shrink-0 lg:transition-none"
>
  <slot />
</aside>
