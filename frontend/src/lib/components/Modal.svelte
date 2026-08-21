<script>
  import { createEventDispatcher, onDestroy } from 'svelte';
  import { fly, fade } from 'svelte/transition';
  import Spinner from './Spinner.svelte';

  export let open = false;
  export let title = '';
  export let subtitle = '';
  /** sm | md | lg | xl */
  export let size = 'md';
  /** While true the modal refuses to close and shows a spinner in the header. */
  export let busy = false;
  /** Clicking the backdrop (or pressing Escape) dismisses the modal. */
  export let dismissible = true;

  const dispatch = createEventDispatcher();

  const widths = {
    sm: 'max-w-sm',
    md: 'max-w-lg',
    lg: 'max-w-2xl',
    xl: 'max-w-4xl',
  };

  let panel;
  let previouslyFocused = null;

  const FOCUSABLE =
    'a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]),' +
    'select:not([disabled]), [contenteditable="true"], [tabindex]:not([tabindex="-1"])';

  function requestClose() {
    if (busy || !dismissible) return;
    dispatch('close');
  }

  function onKeydown(event) {
    if (!open) return;

    if (event.key === 'Escape') {
      event.stopPropagation();
      requestClose();
      return;
    }

    // Keep Tab inside the dialog — otherwise focus wanders into the page behind
    // the backdrop, which screen readers and keyboard users experience as the
    // modal simply not being there.
    if (event.key !== 'Tab' || !panel) return;
    const items = [...panel.querySelectorAll(FOCUSABLE)].filter(
      (el) => el.offsetParent !== null || el === document.activeElement
    );
    if (items.length === 0) {
      event.preventDefault();
      return;
    }
    const first = items[0];
    const last = items[items.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  // Move focus in on open, put it back where it was on close.
  function trap(node) {
    previouslyFocused = document.activeElement;
    const target =
      node.querySelector('[data-autofocus]') ??
      node.querySelector(FOCUSABLE) ??
      node;
    // Wait a frame so transitions don't fight the focus call.
    requestAnimationFrame(() => target?.focus?.({ preventScroll: true }));
    return {
      destroy() {
        previouslyFocused?.focus?.({ preventScroll: true });
        previouslyFocused = null;
      },
    };
  }

  onDestroy(() => previouslyFocused?.focus?.({ preventScroll: true }));
</script>

<svelte:window on:keydown={onKeydown} />

{#if open}
  <div
    class="fixed inset-0 z-50 flex items-end sm:items-center justify-center
           p-0 sm:p-4 bg-gray-900/40 dark:bg-black/60 backdrop-blur-[2px]"
    transition:fade={{ duration: 120 }}
    on:click|self={requestClose}
    role="presentation"
  >
    <div
      bind:this={panel}
      use:trap
      transition:fly={{ y: 12, duration: 180 }}
      class="w-full {widths[size] ?? widths.md} flex flex-col overflow-hidden
             max-h-[92vh] sm:max-h-[90vh]
             rounded-t-2xl sm:rounded-2xl bg-white dark:bg-gray-800
             shadow-2xl shadow-gray-900/20 dark:shadow-black/50
             ring-1 ring-gray-900/5 dark:ring-white/10"
      role="dialog"
      aria-modal="true"
      aria-label={title || undefined}
      tabindex="-1"
    >
      {#if title || $$slots.header}
        <header
          class="flex items-start justify-between gap-4 px-5 py-4 flex-shrink-0
                 border-b border-gray-200 dark:border-gray-700"
        >
          <slot name="header">
            <div class="min-w-0">
              <h2 class="text-sm font-semibold text-gray-900 dark:text-gray-100 truncate">
                {title}
              </h2>
              {#if subtitle}
                <p class="text-xs text-gray-500 dark:text-gray-400 mt-0.5">{subtitle}</p>
              {/if}
            </div>
          </slot>

          <div class="flex items-center gap-2 flex-shrink-0">
            {#if busy}
              <Spinner size="sm" label="Working" />
            {/if}
            <button
              type="button"
              on:click={requestClose}
              disabled={busy || !dismissible}
              aria-label="Close dialog"
              class="p-1 -m-1 rounded-lg text-gray-400
                     hover:text-gray-600 dark:hover:text-gray-200
                     hover:bg-gray-100 dark:hover:bg-gray-700
                     focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500
                     disabled:opacity-40 disabled:cursor-not-allowed
                     transition-colors duration-150"
            >
              <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                   stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" />
              </svg>
            </button>
          </div>
        </header>
      {/if}

      <div class="flex-1 min-h-0 overflow-y-auto">
        <slot />
      </div>

      {#if $$slots.footer}
        <footer
          class="flex items-center justify-between gap-3 px-5 py-3 flex-shrink-0
                 border-t border-gray-200 dark:border-gray-700
                 bg-gray-50/60 dark:bg-gray-800/60"
        >
          <slot name="footer" />
        </footer>
      {/if}
    </div>
  </div>
{/if}
