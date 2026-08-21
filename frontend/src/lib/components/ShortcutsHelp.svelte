<script>
  import Modal from './Modal.svelte';
  import { shortcutsOpen } from '$lib/stores/mail.js';

  const GROUPS = [
    {
      title: 'Navigate',
      items: [
        { keys: ['j'], alt: '↓', label: 'Next message' },
        { keys: ['k'], alt: '↑', label: 'Previous message' },
        { keys: ['/'], label: 'Search' },
        { keys: ['Esc'], label: 'Clear selection or close the message' },
      ],
    },
    {
      title: 'Act on a message',
      items: [
        { keys: ['r'], label: 'Reply' },
        { keys: ['a'], label: 'Reply all' },
        { keys: ['f'], label: 'Forward' },
        { keys: ['u'], label: 'Toggle read / unread' },
        { keys: ['#'], label: 'Delete' },
      ],
    },
    {
      title: 'Everything else',
      items: [
        { keys: ['⌘', 'K'], label: 'Command palette — jump anywhere' },
        { keys: ['c'], label: 'Compose' },
        { keys: ['.'], label: 'Refresh' },
        { keys: ['?'], label: 'Show this help' },
      ],
    },
  ];

  const keyCls =
    'inline-flex items-center justify-center min-w-[1.6rem] h-6 px-1.5 rounded-md ' +
    'text-[11px] font-semibold font-mono ' +
    'bg-gray-100 dark:bg-gray-700 text-gray-700 dark:text-gray-200 ' +
    'border-b-2 border-gray-300 dark:border-gray-600';
</script>

<Modal
  open={$shortcutsOpen}
  title="Keyboard shortcuts"
  subtitle="Press ? at any time to open this list"
  size="lg"
  on:close={() => shortcutsOpen.set(false)}
>
  <div class="grid sm:grid-cols-2 gap-x-8 gap-y-6 px-5 py-5">
    {#each GROUPS as group}
      <section>
        <h3 class="text-xs font-semibold uppercase tracking-wider text-gray-400 dark:text-gray-500 mb-2.5">
          {group.title}
        </h3>
        <dl class="space-y-2">
          {#each group.items as item}
            <div class="flex items-center justify-between gap-4">
              <dt class="text-sm text-gray-700 dark:text-gray-300">{item.label}</dt>
              <dd class="flex items-center gap-1 flex-shrink-0">
                {#each item.keys as key}
                  <kbd class={keyCls}>{key}</kbd>
                {/each}
                {#if item.alt}
                  <span class="text-xs text-gray-400 dark:text-gray-500">or</span>
                  <kbd class={keyCls}>{item.alt}</kbd>
                {/if}
              </dd>
            </div>
          {/each}
        </dl>
      </section>
    {/each}
  </div>
</Modal>
