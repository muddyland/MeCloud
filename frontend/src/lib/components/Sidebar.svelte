<script>
  import {
    mailboxes, selectedMailbox, newFolderOpen,
    draggedEmailId, emails, selectedEmailId, selectedEmailIds, jmapAccountId
  } from '$lib/stores/mail.js';
  import { moveEmail, bulkMove, renameMailbox, deleteMailbox } from '$lib/api.js';
  import { refreshMailboxCounts } from '$lib/mailboxRefresh.js';
  import { toast } from '$lib/stores/toast.js';
  import MailboxIcon from './MailboxIcon.svelte';
  import AppNav from './AppNav.svelte';

  let dragOverId  = null;
  let renamingId  = null;
  let renameValue = '';
  let renameError = '';
  let renaming    = false;
  let deletingId  = null;
  let deleting    = false;
  let collapsed   = new Set(); // folder IDs whose children are hidden

  function toggleCollapse(id) {
    const next = new Set(collapsed);
    if (next.has(id)) next.delete(id); else next.add(id);
    collapsed = next;
  }

  // Build a flat, depth-annotated list of non-role folders that respects parentId.
  // Folders whose parent is null or a role-mailbox are treated as roots here.
  $: flatFolders = (() => {
    const roleMbIds = new Set($mailboxes.filter(m => m.role).map(m => m.id));
    const nonRole   = $mailboxes.filter(m => !m.role);

    const childMap = new Map();
    for (const mb of nonRole) {
      const effectiveRoot = !mb.parentId || roleMbIds.has(mb.parentId);
      const pid = effectiveRoot ? null : mb.parentId;
      if (!childMap.has(pid)) childMap.set(pid, []);
      childMap.get(pid).push(mb);
    }

    const result = [];
    function visit(pid, depth) {
      for (const mb of childMap.get(pid) ?? []) {
        const hasChildren = (childMap.get(mb.id) ?? []).length > 0;
        result.push({ mailbox: mb, depth, hasChildren });
        if (hasChildren && !collapsed.has(mb.id)) visit(mb.id, depth + 1);
      }
    }
    visit(null, 0);
    return result;
  })();

  function startRename(mailbox) {
    renamingId  = mailbox.id;
    renameValue = mailbox.name;
    renameError = '';
  }

  function cancelRename() {
    renamingId = null;
    renameValue = '';
    renameError = '';
  }

  async function confirmRename(mailbox) {
    const trimmed = renameValue.trim();
    if (!trimmed || trimmed === mailbox.name) { cancelRename(); return; }
    renaming = true;
    renameError = '';
    try {
      await renameMailbox($jmapAccountId, mailbox.id, trimmed);
      mailboxes.update(list => list.map(m => m.id === mailbox.id ? { ...m, name: trimmed } : m));
      if ($selectedMailbox?.id === mailbox.id) {
        selectedMailbox.update(m => m ? { ...m, name: trimmed } : m);
      }
      toast('Folder renamed', 'success');
      cancelRename();
    } catch (e) {
      renameError = e?.message ?? 'Rename failed';
    } finally {
      renaming = false;
    }
  }

  function startDelete(mailbox) { deletingId = mailbox.id; }
  function cancelDelete()       { deletingId = null; }

  async function confirmDelete(mailbox) {
    deleting = true;
    try {
      await deleteMailbox($jmapAccountId, mailbox.id);
      const remaining = $mailboxes.filter(m => m.id !== mailbox.id);
      mailboxes.set(remaining);
      if ($selectedMailbox?.id === mailbox.id) {
        selectedMailbox.set(remaining.find(m => m.role === 'inbox') ?? remaining[0] ?? null);
      }
      toast('Folder deleted', 'success');
      deletingId = null;
    } catch (e) {
      toast(e?.message ?? 'Delete failed', 'error');
      deletingId = null;
    } finally {
      deleting = false;
    }
  }

  function handleRenameKey(e, mailbox) {
    if (e.key === 'Enter')  confirmRename(mailbox);
    if (e.key === 'Escape') cancelRename();
  }

  async function dropEmail(e, mailbox) {
    e.preventDefault();
    dragOverId = null;
    if (!$draggedEmailId) return;
    const dragged  = $draggedEmailId;
    const sourceId = $selectedMailbox?.id;
    draggedEmailId.set(null);
    try {
      if (Array.isArray(dragged)) {
        await bulkMove($jmapAccountId, dragged, mailbox.id, sourceId);
        emails.update(l => l.filter(em => !dragged.includes(em.id)));
        if (dragged.includes($selectedEmailId)) selectedEmailId.set(null);
        selectedEmailIds.set(new Set());
        toast(`${dragged.length} messages moved to ${mailbox.name}`, 'success');
      } else {
        await moveEmail($jmapAccountId, dragged, mailbox.id, sourceId);
        emails.update(l => l.filter(em => em.id !== dragged));
        if ($selectedEmailId === dragged) selectedEmailId.set(null);
        toast(`Moved to ${mailbox.name}`, 'success');
      }
      await refreshMailboxCounts();
    } catch (e) {
      toast(e?.message ?? 'Move failed', 'error');
    }
  }
</script>

<aside class="flex flex-col h-full w-full bg-gray-100 dark:bg-gray-900">

  <nav class="flex-1 overflow-y-auto px-2 py-1">

    <!-- ── Mailboxes (system roles) ─────────────────────────────── -->
    <div class="px-1 pt-2 pb-1">
      <span class="text-xs font-semibold text-gray-400 dark:text-gray-500 uppercase tracking-wider select-none">
        Mailboxes
      </span>
    </div>

    {#each $mailboxes.filter(m => m.role) as mailbox (mailbox.id)}
      {@const isSelected = $selectedMailbox?.id === mailbox.id}
      {@const isDragOver = $draggedEmailId !== null && dragOverId === mailbox.id}

      <div class="relative group rounded-lg mb-0.5">
        <button
          on:click={() => selectedMailbox.set(mailbox)}
          on:dragover={(e) => { e.preventDefault(); dragOverId = mailbox.id; }}
          on:dragleave={() => { dragOverId = null; }}
          on:drop={(e) => dropEmail(e, mailbox)}
          class="w-full flex items-center justify-between px-3 py-2 rounded-lg text-sm
                 transition-colors duration-150
                 {isSelected
                   ? 'bg-blue-100 dark:bg-blue-900/50 text-blue-700 dark:text-blue-300 font-medium'
                   : 'text-gray-700 dark:text-gray-300 hover:bg-gray-200 dark:hover:bg-gray-800'}
                 {isDragOver ? 'ring-2 ring-blue-400 dark:ring-blue-500' : ''}"
        >
          <span class="flex items-center gap-2 truncate">
            <MailboxIcon role={mailbox.role} cls="w-4 h-4 flex-shrink-0 opacity-60" />
            <span class="truncate">{mailbox.name}</span>
          </span>
          {#if mailbox.unreadEmails > 0}
            <span class="ml-1 text-xs font-semibold px-1.5 py-0.5 rounded-full
                         bg-blue-500 dark:bg-blue-600 text-white">
              {mailbox.unreadEmails}
            </span>
          {/if}
        </button>
      </div>
    {/each}

    <!-- ── Folders (user-created, hierarchical) ─────────────────── -->
    <div class="px-1 pt-3 pb-1 flex items-center justify-between">
      <span class="text-xs font-semibold text-gray-400 dark:text-gray-500 uppercase tracking-wider select-none">
        Folders
      </span>
      <button
        on:click={() => newFolderOpen.set(true)}
        title="New folder"
        class="w-5 h-5 flex items-center justify-center rounded text-gray-400 dark:text-gray-500
               hover:text-gray-600 dark:hover:text-gray-300 hover:bg-gray-200 dark:hover:bg-gray-700
               transition-colors duration-150 text-base leading-none"
      >+</button>
    </div>

    {#each flatFolders as { mailbox, depth, hasChildren } (mailbox.id)}
      {@const isSelected = $selectedMailbox?.id === mailbox.id}
      {@const isDragOver = $draggedEmailId !== null && dragOverId === mailbox.id}

      <div class="relative group rounded-lg mb-0.5" style="margin-left: {depth * 12}px">

        {#if renamingId === mailbox.id}
          <!-- ── Inline rename ───────────────────────────────────── -->
          <div class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg
                      bg-blue-50 dark:bg-blue-900/30 border border-blue-200 dark:border-blue-700/50">
            <MailboxIcon role={mailbox.role} cls="w-4 h-4 flex-shrink-0 text-gray-500 dark:text-gray-400" />
            <input
              bind:value={renameValue}
              on:keydown={(e) => handleRenameKey(e, mailbox)}
              disabled={renaming}
              class="flex-1 min-w-0 text-sm bg-transparent text-gray-800 dark:text-gray-100
                     border-b border-blue-400 dark:border-blue-500 focus:outline-none py-0.5"
              autofocus
            />
            <button
              on:click={() => confirmRename(mailbox)}
              disabled={renaming}
              class="text-xs text-blue-600 dark:text-blue-400 hover:text-blue-700 font-medium
                     flex-shrink-0 disabled:opacity-50"
            >✓</button>
            <button
              on:click={cancelRename}
              class="text-xs text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 flex-shrink-0"
            >✕</button>
          </div>
          {#if renameError}
            <p class="text-xs text-red-500 px-3 pt-0.5">{renameError}</p>
          {/if}

        {:else}
          <!-- ── Normal row ──────────────────────────────────────── -->
          <div class="flex items-center w-full">

            <!-- Collapse chevron or spacer -->
            {#if hasChildren}
              <button
                on:click|stopPropagation={() => toggleCollapse(mailbox.id)}
                title={collapsed.has(mailbox.id) ? 'Expand' : 'Collapse'}
                class="flex-shrink-0 flex items-center justify-center w-5 h-8
                       text-gray-400 dark:text-gray-500
                       hover:text-gray-600 dark:hover:text-gray-300
                       transition-colors duration-100 rounded"
              >
                <svg
                  class="w-2.5 h-2.5 transition-transform duration-150"
                  style="transform: rotate({collapsed.has(mailbox.id) ? '0deg' : '90deg'})"
                  viewBox="0 0 24 24" fill="none" stroke="currentColor"
                  stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"
                >
                  <polyline points="9 18 15 12 9 6"/>
                </svg>
              </button>
            {:else}
              <div class="w-5 flex-shrink-0"></div>
            {/if}

            <!-- Folder button -->
            <button
              on:click={() => selectedMailbox.set(mailbox)}
              on:dragover={(e) => { e.preventDefault(); dragOverId = mailbox.id; }}
              on:dragleave={() => { dragOverId = null; }}
              on:drop={(e) => dropEmail(e, mailbox)}
              class="flex-1 min-w-0 flex items-center justify-between px-2 py-2 rounded-lg text-sm
                     transition-colors duration-150
                     {isSelected
                       ? 'bg-blue-100 dark:bg-blue-900/50 text-blue-700 dark:text-blue-300 font-medium'
                       : 'text-gray-700 dark:text-gray-300 hover:bg-gray-200 dark:hover:bg-gray-800'}
                     {isDragOver ? 'ring-2 ring-blue-400 dark:ring-blue-500' : ''}"
            >
              <span class="flex items-center gap-2 truncate">
                <MailboxIcon role={mailbox.role} cls="w-4 h-4 flex-shrink-0 opacity-60" />
                <span class="truncate">{mailbox.name}</span>
              </span>
              {#if mailbox.unreadEmails > 0}
                <span class="ml-1 text-xs font-semibold px-1.5 py-0.5 rounded-full
                             bg-blue-500 dark:bg-blue-600 text-white
                             group-hover:invisible">
                  {mailbox.unreadEmails}
                </span>
              {/if}
            </button>
          </div>

          <!-- Hover action buttons -->
          <div class="absolute right-1 inset-y-0 hidden group-hover:flex items-center gap-0.5 pr-0.5">
            {#if deletingId === mailbox.id}
              <button
                on:click|stopPropagation={() => confirmDelete(mailbox)}
                disabled={deleting}
                class="text-xs px-1.5 py-0.5 rounded bg-red-500 hover:bg-red-600 text-white
                       disabled:opacity-50 transition-colors duration-100"
              >{deleting ? '…' : 'Delete?'}</button>
              <button
                on:click|stopPropagation={cancelDelete}
                class="text-xs px-1 py-0.5 text-gray-400 hover:text-gray-600 dark:hover:text-gray-300"
              >✕</button>
            {:else}
              <button
                on:click|stopPropagation={() => startRename(mailbox)}
                title="Rename folder"
                class="p-1 text-gray-400 hover:text-gray-600 dark:hover:text-gray-300
                       hover:bg-gray-300 dark:hover:bg-gray-700 rounded
                       transition-colors duration-100"
              >
                <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                  <path d="m16.862 4.487 1.687-1.688a1.875 1.875 0 112.652 2.652L6.832 19.82a4.5 4.5 0 01-1.897 1.13l-2.685.8.8-2.685a4.5 4.5 0 011.13-1.897L16.863 4.487zm0 0L19.5 7.125"/>
                </svg>
              </button>
              <button
                on:click|stopPropagation={() => startDelete(mailbox)}
                title="Delete folder"
                class="p-1 text-gray-400 hover:text-red-500 dark:hover:text-red-400
                       hover:bg-gray-300 dark:hover:bg-gray-700 rounded
                       transition-colors duration-100"
              >
                <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                  <path d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 01-2.244 2.077H8.084a2.25 2.25 0 01-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 00-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 013.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 00-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 00-7.5 0"/>
                </svg>
              </button>
            {/if}
          </div>

        {/if}
      </div>
    {/each}

  </nav>

  <AppNav />

</aside>
