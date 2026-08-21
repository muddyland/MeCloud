<script>
  import FileIcon from './FileIcon.svelte';
  import Spinner from './Spinner.svelte';
  import { childrenByParent, currentFolderId, expandedFolders } from '$lib/stores/files.js';
  import { fileKind } from '$lib/fileTypes.js';

  /** null renders the top level; a folder id renders that folder's children. */
  export let parentId = null;
  export let depth = 0;
  /** (nodeId, targetFolderId) => void — invoked on a drop. */
  export let onMove = () => {};
  /** Folder id currently being moved, so it can show a spinner. */
  export let busyId = null;

  let dragOverId = null;

  $: folders = ($childrenByParent.get(parentId) ?? []).filter((n) => fileKind(n) === 'folder');

  function hasChildFolders(id) {
    return ($childrenByParent.get(id) ?? []).some((n) => fileKind(n) === 'folder');
  }

  function toggle(id) {
    expandedFolders.update((s) => {
      const next = new Set(s);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  }

  function handleDrop(event, folder) {
    dragOverId = null;
    const id = event.dataTransfer?.getData('text/jmap-filenode');
    if (id) onMove(id, folder.id);
  }
</script>

{#each folders as folder (folder.id)}
  {@const expanded = $expandedFolders.has(folder.id)}
  {@const expandable = hasChildFolders(folder.id)}
  {@const selected = $currentFolderId === folder.id}

  <div class="flex items-center" style="padding-left: {depth * 12}px">
    {#if expandable}
      <button
        on:click|stopPropagation={() => toggle(folder.id)}
        aria-label={expanded ? 'Collapse' : 'Expand'}
        class="flex-shrink-0 w-5 h-8 flex items-center justify-center rounded
               text-gray-400 hover:text-gray-600 dark:hover:text-gray-300
               transition-colors duration-100"
      >
        <svg class="w-2.5 h-2.5 transition-transform duration-150"
             style="transform: rotate({expanded ? '90deg' : '0deg'})"
             viewBox="0 0 24 24" fill="none" stroke="currentColor"
             stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
          <path d="m9 6 6 6-6 6" />
        </svg>
      </button>
    {:else}
      <div class="w-5 flex-shrink-0"></div>
    {/if}

    <button
      on:click={() => currentFolderId.set(folder.id)}
      on:dragover|preventDefault={() => (dragOverId = folder.id)}
      on:dragleave={() => (dragOverId = null)}
      on:drop|preventDefault={(e) => handleDrop(e, folder)}
      class="flex-1 min-w-0 flex items-center gap-2 px-2 py-1.5 rounded-lg text-sm
             transition-colors duration-150
             {selected
               ? 'bg-blue-100 dark:bg-blue-900/50 text-blue-700 dark:text-blue-300 font-medium'
               : 'text-gray-700 dark:text-gray-300 hover:bg-gray-200 dark:hover:bg-gray-800'}
             {dragOverId === folder.id ? 'ring-2 ring-blue-400 dark:ring-blue-500' : ''}"
    >
      {#if busyId === folder.id}
        <Spinner size="xs" label="" />
      {:else}
        <FileIcon node={folder} size="sm" />
      {/if}
      <span class="truncate">{folder.name}</span>
    </button>
  </div>

  {#if expanded}
    <svelte:self parentId={folder.id} depth={depth + 1} {onMove} {busyId} />
  {/if}
{/each}
