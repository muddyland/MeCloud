<script>
  import { fly } from 'svelte/transition';
  import { movePickerOpen, mailboxes, selectedMailbox, emails, selectedEmailId, jmapAccountId } from '$lib/stores/mail.js';
  import { moveEmail } from '$lib/api.js';
  import { toast } from '$lib/stores/toast.js';
  import MailboxIcon from './MailboxIcon.svelte';

  let moving = false;
  let error = '';

  $: otherMailboxes = $mailboxes.filter(m => m.id !== $selectedMailbox?.id);

  async function pick(mailbox) {
    if (moving) return;
    moving = true;
    error = '';
    const emailId = $movePickerOpen;
    try {
      await moveEmail($jmapAccountId, emailId, mailbox.id, $selectedMailbox?.id);
      emails.update(list => list.filter(e => e.id !== emailId));
      if ($selectedEmailId === emailId) selectedEmailId.set(null);
      toast(`Moved to ${mailbox.name}`, 'success');
      movePickerOpen.set(null);
    } catch (e) {
      error = e.message || 'Move failed';
    } finally {
      moving = false;
    }
  }
</script>

{#if $movePickerOpen !== null}
  <div class="fixed inset-0 z-40 bg-black/30 dark:bg-black/50"
       on:click={() => !moving && movePickerOpen.set(null)}
       aria-hidden="true"></div>

  <div
    class="fixed z-50 top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2
           w-72 rounded-xl shadow-xl ring-1
           bg-white dark:bg-gray-800 ring-black/5 dark:ring-white/10 overflow-hidden"
    in:fly={{ y: 8, duration: 150 }}
  >
    <div class="px-4 py-3 border-b border-gray-100 dark:border-gray-700">
      <p class="text-sm font-semibold text-gray-800 dark:text-gray-100">
        {moving ? 'Moving…' : 'Move to folder'}
      </p>
    </div>

    <div class="max-h-64 overflow-y-auto py-1">
      {#each otherMailboxes as mailbox}
        <button
          on:click={() => pick(mailbox)}
          disabled={moving}
          class="flex items-center gap-2.5 w-full px-4 py-2 text-sm text-gray-700 dark:text-gray-300
                 hover:bg-gray-50 dark:hover:bg-gray-700/60 transition-colors duration-100
                 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          <MailboxIcon role={mailbox.role} cls="w-4 h-4 opacity-60 flex-shrink-0" />
          {mailbox.name}
        </button>
      {/each}
    </div>

    {#if error}
      <p class="px-4 pb-2 text-xs text-red-500">{error}</p>
    {/if}

    <div class="px-4 py-2 border-t border-gray-100 dark:border-gray-700">
      <button
        on:click={() => movePickerOpen.set(null)}
        disabled={moving}
        class="text-xs text-gray-500 hover:text-gray-700 dark:hover:text-gray-300"
      >Cancel</button>
    </div>
  </div>
{/if}
