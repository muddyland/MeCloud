<script>
  /**
   * Pick files already in the drive, to attach to a message.
   *
   * Deliberately keeps its own folder and selection state rather than reusing
   * `currentFolderId` / `selectedFileIds`: compose can be opened *from* the
   * Files page, and browsing in here must not renavigate the listing behind it
   * or silently redefine what "selected" means once the dialog closes.
   *
   * The node list itself is the shared `fileNodes` cache — read, and populated
   * on demand when compose was opened from an app that never loaded it.
   */
  import { createEventDispatcher } from 'svelte';
  import Modal from './Modal.svelte';
  import Spinner from './Spinner.svelte';
  import FileThumbnail from './FileThumbnail.svelte';
  import { fileNodes, nodesById, childrenByParent } from '$lib/stores/files.js';
  import { fetchFileNodes, supportsFiles } from '$lib/files.js';
  import { jmapAccountId, jmapSession } from '$lib/stores/mail.js';
  import { breadcrumbTrail, compareNodes, fileKind, formatBytes } from '$lib/fileTypes.js';

  export let open = false;
  /** Blob ids already on the message, so they can be shown as attached. */
  export let attachedBlobIds = new Set();

  const dispatch = createEventDispatcher();

  let folderId = null;
  let selected = new Set();
  let search = '';
  let loading = false;
  let error = '';
  let incomplete = false;
  let loaded = false;

  $: supported = supportsFiles($jmapSession);

  // Load once per open, and only when nothing else has already fetched the tree.
  $: if (open && supported && !loaded) {
    loaded = true;
    if ($fileNodes.length === 0) load();
  }
  $: if (!open) reset();

  function reset() {
    folderId = null;
    selected = new Set();
    search = '';
    error = '';
    loaded = false;
  }

  async function load() {
    loading = true;
    error = '';
    try {
      const result = await fetchFileNodes($jmapAccountId, $jmapSession);
      fileNodes.set(result.nodes);
      incomplete = result.incomplete;
    } catch (e) {
      error = e?.message ?? 'Could not load your files.';
    } finally {
      loading = false;
    }
  }

  $: query = search.trim().toLowerCase();
  // Search spans the whole drive, matching the Files app — scoping it to the
  // open folder makes it useless for actually finding an attachment.
  $: rows = (query
      ? $fileNodes.filter((n) => String(n.name ?? '').toLowerCase().includes(query))
      : ($childrenByParent.get(folderId ?? null) ?? [])
    ).slice().sort((a, b) => compareNodes(a, b, { key: 'name', ascending: true }));

  $: trail = breadcrumbTrail($nodesById, folderId);
  $: chosen = [...selected].map((id) => $nodesById.get(id)).filter(Boolean);
  $: chosenBytes = chosen.reduce((sum, n) => sum + (Number(n.size) || 0), 0);

  const isFolder = (n) => fileKind(n) === 'folder';
  const alreadyAttached = (n) => attachedBlobIds.has(n.blobId);

  function activate(node) {
    if (isFolder(node)) {
      folderId = node.id;
      search = '';
      return;
    }
    if (alreadyAttached(node)) return;
    const next = new Set(selected);
    if (next.has(node.id)) next.delete(node.id); else next.add(node.id);
    selected = next;
  }

  function attach() {
    if (!chosen.length) return;
    dispatch('attach', chosen);
  }
</script>

<Modal
  {open}
  title="Attach from Files"
  subtitle={trail.length ? trail.map((c) => c.name).join(' / ') : 'Files'}
  size="lg"
  on:close={() => dispatch('close')}
>
  <div class="flex flex-col">
    <!-- Breadcrumbs + search -->
    <div class="flex items-center gap-2 px-4 py-2 border-b border-gray-200 dark:border-gray-700
                flex-shrink-0 sticky top-0 bg-white dark:bg-gray-800 z-10">
      <nav class="flex items-center gap-1 min-w-0 flex-1 text-sm" aria-label="Breadcrumb">
        <button on:click={() => { folderId = null; search = ''; }}
          class="px-1.5 py-0.5 rounded text-gray-600 dark:text-gray-300
                 hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors duration-100">
          Files
        </button>
        {#each trail as crumb (crumb.id)}
          <span class="text-gray-300 dark:text-gray-600 select-none">/</span>
          <button on:click={() => { folderId = crumb.id; search = ''; }}
            class="px-1.5 py-0.5 rounded truncate max-w-[10rem] text-gray-600 dark:text-gray-300
                   hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors duration-100">
            {crumb.name}
          </button>
        {/each}
      </nav>

      <div class="relative flex items-center flex-shrink-0">
        <svg class="absolute left-2.5 w-3.5 h-3.5 text-gray-400 pointer-events-none"
             viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" stroke-linejoin="round">
          <circle cx="11" cy="11" r="8" /><path d="m21 21-4.35-4.35" />
        </svg>
        <input
          bind:value={search}
          placeholder="Search files…"
          class="w-40 text-xs pl-8 pr-2 py-1.5 rounded-lg
                 bg-gray-100 dark:bg-gray-700/60 text-gray-800 dark:text-gray-200
                 placeholder-gray-400 dark:placeholder-gray-500
                 border border-transparent focus:border-blue-400 dark:focus:border-blue-500
                 focus:outline-none transition-colors duration-150"
        />
      </div>
    </div>

    <!-- Listing -->
    <div class="min-h-[18rem]">
      {#if !supported}
        <p class="text-sm text-gray-500 dark:text-gray-400 text-center py-16 px-6">
          This server does not advertise JMAP file storage, so there is nothing to attach from.
        </p>
      {:else if loading}
        <div class="flex items-center justify-center gap-2 py-16 text-sm text-gray-500 dark:text-gray-400">
          <Spinner size="sm" label="" /> Loading your files…
        </div>
      {:else if error}
        <div class="flex flex-col items-center gap-3 py-16">
          <p class="text-sm text-gray-500 dark:text-gray-400">{error}</p>
          <button on:click={load}
            class="text-xs font-medium px-3 py-1.5 rounded-lg bg-gray-100 dark:bg-gray-700
                   hover:bg-gray-200 dark:hover:bg-gray-600 text-gray-700 dark:text-gray-200
                   transition-colors duration-150">Try again</button>
        </div>
      {:else if rows.length === 0}
        <p class="text-sm text-gray-400 dark:text-gray-500 text-center py-16">
          {query ? 'No files match that search' : 'This folder is empty'}
        </p>
      {:else}
        <ul class="divide-y divide-gray-100 dark:divide-gray-700/60">
          {#each rows as node (node.id)}
            {@const folder = isFolder(node)}
            {@const attached = !folder && alreadyAttached(node)}
            {@const picked = selected.has(node.id)}
            <li>
              <button
                on:click={() => activate(node)}
                disabled={attached}
                class="flex items-center gap-3 w-full px-4 py-2 text-left
                       transition-colors duration-100
                       disabled:cursor-not-allowed
                       {picked ? 'bg-blue-50 dark:bg-blue-900/30'
                               : 'hover:bg-gray-50 dark:hover:bg-gray-700/40'}"
              >
                {#if folder}
                  <span class="w-4 h-4 flex-shrink-0"></span>
                {:else}
                  <span
                    class="w-4 h-4 rounded border-2 flex items-center justify-center flex-shrink-0
                           {picked ? 'bg-blue-500 border-blue-500'
                                   : 'border-gray-300 dark:border-gray-500'}
                           {attached ? 'opacity-40' : ''}"
                  >
                    {#if picked}
                      <svg class="w-2.5 h-2.5 text-white" viewBox="0 0 24 24" fill="none"
                           stroke="currentColor" stroke-width="3.5" stroke-linecap="round"
                           stroke-linejoin="round"><path d="m20 6-11 11-5-5" /></svg>
                    {/if}
                  </span>
                {/if}

                <FileThumbnail {node} size="sm" />

                <span class="flex-1 min-w-0 truncate text-sm
                             {attached ? 'text-gray-400 dark:text-gray-500'
                                       : 'text-gray-800 dark:text-gray-200'}">
                  {node.name}
                </span>

                {#if attached}
                  <span class="text-xs text-gray-400 dark:text-gray-500 flex-shrink-0">Attached</span>
                {:else if folder}
                  <svg class="w-4 h-4 text-gray-300 dark:text-gray-600 flex-shrink-0"
                       viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round"><path d="m9 18 6-6-6-6" /></svg>
                {:else}
                  <span class="text-xs text-gray-400 dark:text-gray-500 tabular-nums flex-shrink-0">
                    {formatBytes(node.size)}
                  </span>
                {/if}
              </button>
            </li>
          {/each}
        </ul>

        {#if incomplete}
          <p class="px-4 py-2 text-xs text-amber-600 dark:text-amber-400">
            This server could not list the whole drive, so some files may be missing from this view.
          </p>
        {/if}
      {/if}
    </div>
  </div>

  <svelte:fragment slot="footer">
    <span class="text-xs text-gray-500 dark:text-gray-400 truncate min-w-0">
      {chosen.length
        ? `${chosen.length} selected · ${formatBytes(chosenBytes)}`
        : 'Select files to attach'}
    </span>
    <div class="flex items-center gap-2 flex-shrink-0">
      <button on:click={() => dispatch('close')}
        class="px-3 py-1.5 text-sm text-gray-500 dark:text-gray-400
               hover:text-gray-700 dark:hover:text-gray-200 transition-colors duration-150">
        Cancel
      </button>
      <button
        on:click={attach}
        disabled={!chosen.length}
        class="px-4 py-1.5 text-sm font-medium rounded-lg bg-blue-500 hover:bg-blue-600
               text-white disabled:opacity-50 disabled:cursor-not-allowed
               transition-colors duration-150"
      >
        Attach{chosen.length ? ` ${chosen.length}` : ''}
      </button>
    </div>
  </svelte:fragment>
</Modal>
