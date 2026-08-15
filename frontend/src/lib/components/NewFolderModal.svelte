<script>
  import { newFolderOpen, mailboxes, jmapAccountId } from '$lib/stores/mail.js';
  import { createMailbox } from '$lib/api.js';
  import { toast } from '$lib/stores/toast.js';
  import Modal from './Modal.svelte';
  import Spinner from './Spinner.svelte';

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
    if (creating) return;
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
        newFolderOpen.set(false);
      } else {
        error = 'Failed to create folder.';
      }
    } catch (e) {
      error = e?.message ?? 'Failed to create folder.';
    } finally {
      creating = false;
    }
  }

  const inputCls =
    'w-full rounded-lg border border-gray-200 dark:border-gray-600 ' +
    'bg-white dark:bg-gray-700 text-sm text-gray-800 dark:text-gray-100 ' +
    'px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500 ' +
    'disabled:opacity-60 disabled:cursor-not-allowed';
</script>

<Modal open={$newFolderOpen} title="New folder" size="sm" busy={creating} on:close={close}>
  <div class="px-5 py-5 space-y-3">
    <label class="block">
      <span class="sr-only">Folder name</span>
      <input
        type="text"
        placeholder="Folder name"
        bind:value={name}
        disabled={creating}
        data-autofocus
        on:keydown={(e) => { if (e.key === 'Enter') handleCreate(); }}
        class={inputCls}
      />
    </label>

    <label class="block">
      <span class="sr-only">Parent folder</span>
      <select bind:value={parentId} disabled={creating} class={inputCls}>
        <option value="">No parent (top level)</option>
        {#each $mailboxes as mailbox}
          <option value={mailbox.id}>{mailbox.name}</option>
        {/each}
      </select>
    </label>

    {#if error}
      <p class="text-xs text-red-500">{error}</p>
    {/if}
  </div>

  <svelte:fragment slot="footer">
    <span></span>
    <div class="flex items-center gap-2">
      <button
        on:click={close}
        disabled={creating}
        class="text-sm text-gray-500 hover:text-gray-700 dark:hover:text-gray-300
               px-3 py-1.5 rounded-lg hover:bg-gray-100 dark:hover:bg-gray-700
               disabled:opacity-40 transition-colors duration-150"
      >
        Cancel
      </button>
      <button
        on:click={handleCreate}
        disabled={name.trim() === '' || creating}
        class="inline-flex items-center gap-2 text-sm font-medium text-white
               bg-blue-500 hover:bg-blue-600 px-3 py-1.5 rounded-lg
               disabled:opacity-50 disabled:cursor-not-allowed
               transition-colors duration-150"
      >
        {#if creating}
          <Spinner size="xs" label="" accent="border-t-white" cls="border-white/40" />
          Creating…
        {:else}
          Create
        {/if}
      </button>
    </div>
  </svelte:fragment>
</Modal>
