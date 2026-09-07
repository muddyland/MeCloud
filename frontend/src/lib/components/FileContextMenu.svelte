<script>
  /**
   * Right-click menu for the Files listing.
   *
   * Deliberately not the mail ContextMenu: that one is bound to emails,
   * mailboxes and the move picker, and the only thing the two actually share
   * is the shape of a floating menu.
   *
   * The page owns the operations — rename, delete and upload all need state
   * that lives there — so they arrive as callbacks rather than being
   * reimplemented here.
   */
  import { fly } from 'svelte/transition';
  import { fileContextMenu, nodesById, selectedFileIds } from '$lib/stores/files.js';
  import { blobUrl, downloadNode } from '$lib/files.js';
  import { fileKind, formatBytes, isPreviewable } from '$lib/fileTypes.js';

  /** @type {(node: object) => void} */
  export let onOpen = () => {};
  /** @type {(node: object) => void} */
  export let onRename = () => {};
  /** @type {(ids: string[]) => void} */
  export let onDelete = () => {};
  /** @type {(nodes: object[]) => void} */
  export let onShare = () => {};
  export let onUpload = () => {};
  export let onNewFolder = () => {};

  $: menu = $fileContextMenu;
  $: ids  = menu?.ids ?? [];
  $: nodes = ids.map((id) => $nodesById.get(id)).filter(Boolean);
  $: node = nodes.length === 1 ? nodes[0] : null;
  $: isBulk = nodes.length > 1;
  // A right-click on empty space: act on the folder itself.
  $: isBackground = !menu ? false : ids.length === 0;

  $: isFolder = node ? fileKind(node) === 'folder' : false;
  // Folders have no blob, so nothing to download, attach or open in a tab.
  $: shareable = nodes.filter((n) => n.blobId);

  function close() { fileContextMenu.set(null); }

  function onKeydown(e) {
    if (menu && e.key === 'Escape') { e.preventDefault(); close(); }
  }

  // Keep the menu inside the viewport when opened near an edge — off-screen
  // items are unreachable, and the bottom row is the destructive one.
  const MENU_W = 210;
  const MENU_H = 330;
  $: menuX = menu
    ? Math.max(8, Math.min(menu.x, (typeof window !== 'undefined' ? window.innerWidth : 1e4) - MENU_W))
    : 0;
  $: menuY = menu
    ? Math.max(8, Math.min(menu.y, (typeof window !== 'undefined' ? window.innerHeight : 1e4) - MENU_H))
    : 0;

  // The action runs before the menu closes: every handler below reads `node`,
  // `nodes` or `ids`, all derived from the store this clears.
  const run = (fn) => (...args) => { fn(...args); close(); };

  const open      = run(() => node && onOpen(node));
  const rename    = run(() => node && onRename(node));
  const remove    = run(() => onDelete(ids));
  const share     = run(() => onShare(shareable));
  const upload    = run(() => onUpload());
  const newFolder = run(() => onNewFolder());

  const download = run(() => {
    // One <a>.click() per file, which is all the browser gives us — there is no
    // "download these" API. A large selection may hit the browser's own
    // multiple-download prompt; that is the browser's call to make, not ours.
    for (const n of nodes) if (n.blobId) downloadNode(n);
  });

  const selectOnly = run(() => {
    if (node) selectedFileIds.set(new Set([node.id]));
  });

  $: totalBytes = shareable.reduce((sum, n) => sum + (Number(n.size) || 0), 0);

  const itemCls = `flex items-center gap-2.5 w-full px-4 py-2 text-sm text-gray-700 dark:text-gray-300
    hover:bg-gray-50 dark:hover:bg-gray-700/60 transition-colors duration-100 text-left
    disabled:opacity-40 disabled:cursor-not-allowed disabled:hover:bg-transparent`;
  const dangerCls = `flex items-center gap-2.5 w-full px-4 py-2 text-sm text-left
    text-red-600 dark:text-red-400
    hover:bg-red-50 dark:hover:bg-red-900/20 transition-colors duration-100`;
  const divider = 'my-1 border-t border-gray-100 dark:border-gray-700';
</script>

<svelte:window on:keydown={onKeydown} />

{#if menu}
  <!-- svelte-ignore a11y-click-events-have-key-events -->
  <div class="fixed inset-0 z-40" on:click={close} on:contextmenu|preventDefault={close}
       aria-hidden="true"></div>

  <div
    class="fixed z-50 min-w-[200px] rounded-lg shadow-lg ring-1
           bg-white dark:bg-gray-800 ring-black/5 dark:ring-white/10 py-1 overflow-hidden"
    style="left: {menuX}px; top: {menuY}px"
    role="menu"
    in:fly={{ y: -4, duration: 120 }}
  >
    {#if isBackground}
      <button on:click={upload} class={itemCls}>
        <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M12 19V7m0 0-4 4m4-4 4 4M5 5h14"/></svg>
        Upload files…
      </button>
      <button on:click={newFolder} class={itemCls}>
        <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M12 11v5M9.5 13.5h5"/></svg>
        New folder
      </button>

    {:else if isBulk}
      <div class="px-4 py-1.5 text-xs font-medium text-gray-400 dark:text-gray-500 select-none">
        {nodes.length} selected
      </div>
      <div class={divider}></div>

      <button on:click={download} class={itemCls} disabled={!shareable.length}>
        <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v12m0 0-4-4m4 4 4-4M5 19h14"/></svg>
        Download {shareable.length || ''}
      </button>
      <button on:click={share} class={itemCls} disabled={!shareable.length}>
        <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M4 6a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2z"/><path d="m4 7 8 6 8-6"/></svg>
        Share via email
      </button>
      {#if shareable.length}
        <div class="px-4 pb-1.5 text-[11px] text-gray-400 dark:text-gray-500 select-none">
          {formatBytes(totalBytes)} total
        </div>
      {/if}

      <div class={divider}></div>

      <button on:click={remove} class={dangerCls}>
        <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 01-2 2H8a2 2 0 01-2-2L5 6"/><path d="M10 11v6M14 11v6"/><path d="M9 6V4a1 1 0 011-1h4a1 1 0 011 1v2"/></svg>
        Delete
      </button>

    {:else if node}
      <button on:click={open} class={itemCls}>
        {#if isFolder}
          <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>
          Open
        {:else if isPreviewable(node)}
          <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7z"/></svg>
          Preview
        {:else}
          <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v12m0 0-4-4m4 4 4-4M5 19h14"/></svg>
          Download
        {/if}
      </button>

      {#if !isFolder && isPreviewable(node)}
        <a href={blobUrl(node, { inline: true })} target="_blank" rel="noopener noreferrer"
           on:click={close} class={itemCls} role="menuitem">
          <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><path d="M15 3h6v6M10 14 21 3"/></svg>
          Open in new tab
        </a>
      {/if}

      {#if !isFolder}
        <button on:click={download} class={itemCls}>
          <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v12m0 0-4-4m4 4 4-4M5 19h14"/></svg>
          Download
        </button>
        <button on:click={share} class={itemCls}>
          <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M4 6a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2z"/><path d="m4 7 8 6 8-6"/></svg>
          Share via email
        </button>
      {/if}

      <div class={divider}></div>

      <button on:click={rename} class={itemCls}>
        <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4z"/></svg>
        Rename
      </button>
      <button on:click={selectOnly} class={itemCls}>
        <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="4" width="16" height="16" rx="2"/><path d="m9 12 2 2 4-4"/></svg>
        Select
      </button>

      <div class={divider}></div>

      <button on:click={remove} class={dangerCls}>
        <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 01-2 2H8a2 2 0 01-2-2L5 6"/><path d="M10 11v6M14 11v6"/><path d="M9 6V4a1 1 0 011-1h4a1 1 0 011 1v2"/></svg>
        Delete
      </button>
    {/if}
  </div>
{/if}
