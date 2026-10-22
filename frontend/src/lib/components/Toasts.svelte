<script>
  import { fly } from 'svelte/transition';
  import { toasts, dismiss, runToastAction } from '$lib/stores/toast.js';

  const config = {
    success: {
      icon: '✓',
      wrap: 'bg-green-50 dark:bg-green-900/30 border-green-200 dark:border-green-700',
      text: 'text-green-800 dark:text-green-100',
      icon_cls: 'text-green-500 dark:text-green-400',
    },
    error: {
      icon: '✕',
      wrap: 'bg-red-50 dark:bg-red-900/30 border-red-200 dark:border-red-700',
      text: 'text-red-800 dark:text-red-100',
      icon_cls: 'text-red-500 dark:text-red-400',
    },
    info: {
      icon: 'ℹ',
      wrap: 'bg-blue-50 dark:bg-blue-900/30 border-blue-200 dark:border-blue-700',
      text: 'text-blue-800 dark:text-blue-100',
      icon_cls: 'text-blue-500 dark:text-blue-400',
    },
  };
</script>

<div class="fixed bottom-4 right-4 z-60 flex flex-col gap-2 pointer-events-none">
  {#each $toasts as t (t.id)}
    {@const c = config[t.type] ?? config.info}
    <div
      class="pointer-events-auto flex items-center gap-3 px-4 py-3
             rounded-lg border shadow-lg max-w-sm w-full text-sm
             {c.wrap} {c.text}"
      in:fly={{ x: 72, duration: 220 }}
      out:fly={{ x: 72, duration: 160 }}
    >
      <span class="font-bold text-base leading-none shrink-0 {c.icon_cls}">{c.icon}</span>
      <span class="flex-1">{t.message}</span>
      {#if t.action}
        <button
          on:click={() => runToastAction(t)}
          class="font-semibold underline-offset-2 hover:underline shrink-0"
        >{t.action.label}</button>
      {/if}
      <button
        on:click={() => dismiss(t.id)}
        class="opacity-40 hover:opacity-80 transition-opacity text-lg leading-none shrink-0"
        aria-label="Dismiss"
      >×</button>
    </div>
  {/each}
</div>
