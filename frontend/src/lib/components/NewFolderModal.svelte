<script>
  import { fly } from 'svelte/transition';
  import { newFolderOpen, mailboxes, jmapAccountId } from '$lib/stores/mail.js';
  import { createMailbox } from '$lib/api.js';
  import { toast } from '$lib/stores/toast.js';

  let name = '';
  let parentId = '';
  let error = '';
  let creating = false;

  $: if ($newFolderOpen) {
    name = '';
    parentId = '';
    error = '';
  }

  function close() {
    newFolderOpen.set(false);
  }

  async function handleCreate() {
    if (name.trim() === '' || creating) return;
    creating = true;
    error = '';
    try {
      const mailbox = await createMailbox($jmapAccountId, name.trim(), parentId || null);
      if (mailbox) {
        mailboxes.update(list => [...list, mailbox]);
        toast(`Folder "${mailbox.name}" created`, 'success');
        close();
      } else {
        error = 'Failed to create folder.';
      }
    } catch (e) {
      error = e?.message ?? 'Failed to create folder.';
    } finally {
      creating = false;
    }
  }
</script>

{#if $newFolderOpen}
  <!-- Backdrop -->
  <div
    class="fixed inset-0 z-40 bg-black/30 dark:bg-black/50"
    on:click={close}
    role="presentation"
  ></div>

  <!-- Modal -->
  <div
    class="fixed z-50 top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2
           w-80 rounded-xl shadow-xl bg-white dark:bg-gray-800
           ring-1 ring-black/5 dark:ring-white/10 overflow-hidden"
    transition:fly={{ y: 8, duration: 150 }}
    role="dialog"
    aria-modal="true"
    aria-labelledby="new-folder-title"
  >
    <!-- Header -->
    <div class="px-4 py-3 border-b border-gray-200 dark:border-gray-700">
      <h2 id="new-folder-title" class="text-sm font-semibold text-gray-800 dark:text-gray-100">
        New Folder
      </h2>
    </div>

    <!-- Body -->
    <div class="px-4 py-4 space-y-3">
      <input
        type="text"
        placeholder="Folder name"
        bind:value={name}
        class="w-full rounded-lg border border-gray-200 dark:border-gray-600
               bg-white dark:bg-gray-700 text-sm text-gray-800 dark:text-gray-100
               px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500"
      />

      <select
        bind:value={parentId}
        class="w-full rounded-lg border border-gray-200 dark:border-gray-600
               bg-white dark:bg-gray-700 text-sm text-gray-800 dark:text-gray-100
               px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500"
      >
        <option value="">No parent (top level)</option>
        {#each $mailboxes as mailbox}
          <option value={mailbox.id}>{mailbox.name}</option>
        {/each}
      </select>

      {#if error}
        <p class="text-xs text-red-500">{error}</p>
      {/if}
    </div>

    <!-- Footer -->
    <div class="px-4 pb-4 flex justify-end gap-2">
      <button
        on:click={close}
        class="text-sm text-gray-500 hover:text-gray-700 dark:hover:text-gray-300
               px-3 py-1.5 rounded-lg hover:bg-gray-100 dark:hover:bg-gray-700
               transition-colors duration-150"
      >
        Cancel
      </button>
      <button
        on:click={handleCreate}
        disabled={name.trim() === '' || creating}
        class="text-sm font-medium text-white bg-blue-500 hover:bg-blue-600
               px-3 py-1.5 rounded-lg disabled:opacity-50 disabled:cursor-not-allowed
               transition-colors duration-150"
      >
        {creating ? 'Creating…' : 'Create'}
      </button>
    </div>
  </div>
{/if}
