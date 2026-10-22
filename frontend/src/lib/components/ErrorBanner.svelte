<script>
  /** A failure the user should see, with an optional way to try again. */
  export let message = '';
  export let onRetry = null;
</script>

{#if message}
  <div class="flex items-center gap-3 px-4 py-2 shrink-0
              bg-red-50 dark:bg-red-900/25 border-b border-red-200 dark:border-red-800/60"
       role="alert">
    <svg class="w-4 h-4 shrink-0 text-red-500" viewBox="0 0 24 24" fill="none"
         stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
      <circle cx="12" cy="12" r="9" /><path d="M12 8v5M12 16.5h.01" />
    </svg>
    <span class="text-xs text-red-800 dark:text-red-200 flex-1">{message}</span>
    {#if onRetry}
      <button on:click={onRetry}
        class="text-xs font-medium px-2.5 py-1 rounded-md
               bg-red-100 dark:bg-red-800/50 hover:bg-red-200 dark:hover:bg-red-800
               text-red-800 dark:text-red-100 transition-colors duration-150">
        Try again
      </button>
    {/if}
  </div>
{/if}
