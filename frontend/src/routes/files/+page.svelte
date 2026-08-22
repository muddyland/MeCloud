<script>
  import { onMount, onDestroy } from 'svelte';
  import { fly, fade } from 'svelte/transition';
  import Navbar from '$lib/components/Navbar.svelte';
  import Toasts from '$lib/components/Toasts.svelte';
  import Spinner from '$lib/components/Spinner.svelte';
  import FileIcon from '$lib/components/FileIcon.svelte';
  import FolderTree from '$lib/components/FolderTree.svelte';
  import FilePreview from '$lib/components/FilePreview.svelte';
  import SidebarDrawer from '$lib/components/SidebarDrawer.svelte';
  import { isCompact, closeSidebar } from '$lib/stores/viewport.js';
  import { jmapAccountId, jmapSession, currentUser, sidebarWidth } from '$lib/stores/mail.js';
  import { getJMAPSession, getAppConfig } from '$lib/api.js';
  import {
    fileNodes, filesLoading, filesError, currentFolderId, selectedFileIds,
    previewNode, fileSearch, viewMode, sortKey, sortAsc,
    uploads, nodesById, childrenByParent, visibleNodes, totalUsage,
  } from '$lib/stores/files.js';
  import {
    getFileNodes, createFolder, renameNode, moveNode, destroyNodes,
    uploadFile, downloadNode, supportsFiles, fileLimits,
    readDropEntries, buildDropTree, countTreeFiles,
  } from '$lib/files.js';
  import {
    fileKind, formatBytes, isPreviewable, validateName, breadcrumbTrail, uniqueName,
  } from '$lib/fileTypes.js';
  import { toast } from '$lib/stores/toast.js';

  let stalwartUrl = '';
  let supported   = true;
  let fileInput;
  let dragDepth   = 0;          // counter, not a boolean — see onDragEnter
  let renamingId  = null;
  let renameValue = '';
  let renameError = '';
  let busyId      = null;
  let creatingFolder = false;
  let newFolderName  = '';
  let confirmingDelete = false;
  let deleting = false;
  let uploadError = '';

  $: limits = fileLimits($jmapSession);
  $: trail  = breadcrumbTrail($nodesById, $currentFolderId);
  $: searching = $fileSearch.trim().length > 0;
  $: selectionCount = $selectedFileIds.size;

  // ── Loading ───────────────────────────────────────────────────────────────

  async function loadNodes() {
    if (!$jmapAccountId) return;
    filesLoading.set(true);
    filesError.set('');
    try {
      fileNodes.set(await getFileNodes($jmapAccountId, $jmapSession));
    } catch (e) {
      filesError.set(e?.message ?? 'Could not load your files.');
    } finally {
      filesLoading.set(false);
    }
  }

  // ── Navigation ────────────────────────────────────────────────────────────

  function openFolder(id) {
    currentFolderId.set(id);
    closeSidebar();
    selectedFileIds.set(new Set());
    fileSearch.set('');
  }

  function openNode(node) {
    if (fileKind(node) === 'folder') openFolder(node.id);
    else if (isPreviewable(node)) previewNode.set(node);
    else downloadNode(node);
  }

  // ── Selection ─────────────────────────────────────────────────────────────

  function toggleSelect(id, event) {
    event?.stopPropagation();
    selectedFileIds.update((s) => {
      const next = new Set(s);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  }

  function clearSelection() { selectedFileIds.set(new Set()); }

  function selectAll() {
    selectedFileIds.set(new Set($visibleNodes.map((n) => n.id)));
  }

  // ── Mutations ─────────────────────────────────────────────────────────────

  async function submitNewFolder() {
    const name = newFolderName.trim();
    const problem = validateName(name, { maxLength: limits.maxNameLength });
    if (problem) { toast(problem, 'error'); return; }
    creatingFolder = false;
    try {
      // No pre-flight capability check here. An earlier version refused to
      // create at the top level when mayCreateTopLevelFileNode was not exactly
      // true, which blocked something that already worked. The server is the
      // authority on what it will accept, and it now reports refusals visibly.
      const created = await createFolder($jmapAccountId, $jmapSession, name, $currentFolderId);
      if (created) fileNodes.update((list) => [...list, created]);
      toast(`Folder "${name}" created`, 'success');
      await reconcile();
    } catch (e) {
      uploadError = e?.message ?? 'Could not create the folder.';
      toast(uploadError, 'error');
    } finally {
      newFolderName = '';
    }
  }

  function startRename(node) {
    renamingId = node.id;
    renameValue = node.name;
    renameError = '';
  }

  async function commitRename(node) {
    const name = renameValue.trim();
    if (name === node.name) { renamingId = null; return; }
    const problem = validateName(name, { maxLength: limits.maxNameLength });
    if (problem) { renameError = problem; return; }
    busyId = node.id;
    try {
      await renameNode($jmapAccountId, $jmapSession, node.id, name);
      fileNodes.update((list) => list.map((n) => (n.id === node.id ? { ...n, name } : n)));
      renamingId = null;
    } catch (e) {
      renameError = e?.message ?? 'Rename failed.';
    } finally {
      busyId = null;
    }
  }

  async function removeSelected() {
    const ids = [...$selectedFileIds];
    if (!ids.length) return;
    deleting = true;
    try {
      await destroyNodes($jmapAccountId, $jmapSession, ids);
      const gone = new Set(ids);
      // Children of a destroyed folder are gone server-side too, so drop any
      // node whose ancestry now leads to a deleted id.
      fileNodes.update((list) => {
        const byId = new Map(list.map((n) => [n.id, n]));
        const orphaned = (n) => {
          let cur = n.parentId;
          const seen = new Set();
          while (cur && !seen.has(cur)) {
            if (gone.has(cur)) return true;
            seen.add(cur);
            cur = byId.get(cur)?.parentId;
          }
          return false;
        };
        return list.filter((n) => !gone.has(n.id) && !orphaned(n));
      });
      toast(`${ids.length} item${ids.length === 1 ? '' : 's'} deleted`, 'success');
      clearSelection();
    } catch (e) {
      toast(e?.message ?? 'Delete failed.', 'error');
    } finally {
      deleting = false;
      confirmingDelete = false;
    }
  }

  /** Guard against dropping a folder into itself or its own descendant. */
  function isDescendant(candidateId, ancestorId) {
    let cur = candidateId;
    const seen = new Set();
    while (cur && !seen.has(cur)) {
      if (cur === ancestorId) return true;
      seen.add(cur);
      cur = $nodesById.get(cur)?.parentId;
    }
    return false;
  }

  async function moveInto(nodeId, targetFolderId) {
    const node = $nodesById.get(nodeId);
    if (!node || node.parentId === targetFolderId) return;
    if (nodeId === targetFolderId || isDescendant(targetFolderId, nodeId)) {
      toast('A folder cannot be moved inside itself.', 'error');
      return;
    }
    busyId = nodeId;
    try {
      await moveNode($jmapAccountId, $jmapSession, nodeId, targetFolderId);
      fileNodes.update((list) =>
        list.map((n) => (n.id === nodeId ? { ...n, parentId: targetFolderId } : n)));
      const target = targetFolderId ? $nodesById.get(targetFolderId)?.name : 'Files';
      toast(`Moved to ${target}`, 'success');
    } catch (e) {
      toast(e?.message ?? 'Move failed.', 'error');
    } finally {
      busyId = null;
    }
  }

  // ── Uploads ───────────────────────────────────────────────────────────────

  let uploadSeq = 0;

  async function uploadFiles(fileList) {
    const files = [...(fileList ?? [])];
    if (!files.length) return;

    const parentId = $currentFolderId;
    const taken = new Set(($childrenByParent.get(parentId ?? null) ?? []).map((n) => n.name));

    let failures = 0;
    for (const file of files) {
      // Uploading two files with the same name into one folder should not
      // silently produce two identical entries.
      const name = uniqueName(file.name, taken);
      taken.add(name);
      if (!(await uploadOne(file, name, parentId))) failures += 1;
    }
    await reconcile(failures);
  }

  /**
   * Re-read the tree from the server after a batch of writes.
   *
   * Until now the listing was built purely from optimistic inserts, so it
   * asserted success on its own authority: anything the server quietly declined
   * still appeared in the UI and only vanished on the next reload — which is
   * exactly what "uploads do not persist" looks like from the outside. Reading
   * back makes the listing show what is actually stored.
   */
  async function reconcile(failures = 0) {
    try {
      const fresh = await getFileNodes($jmapAccountId, $jmapSession);
      const before = $fileNodes.length;
      fileNodes.set(fresh);
      if (failures === 0 && fresh.length < before) {
        toast('Some items were not saved by the server.', 'error');
      }
    } catch {
      // Leave the optimistic view in place; the next navigation will refresh.
    }
  }

  async function onFilePicked(event) {
    const input = event.currentTarget;
    // Deliberately not cleared until the uploads settle. On iOS the File
    // objects are backed by the photo library, and resetting the input while
    // one is still being read can invalidate it mid-request.
    try {
      await uploadFiles(input.files);
    } finally {
      input.value = '';                    // allow re-picking the same file
    }
  }

  // dragenter/dragleave fire for every child element, so a boolean flag
  // flickers as the pointer crosses them. Counting enters and leaves does not.
  function onDragEnter(e) {
    if (!e.dataTransfer?.types?.includes('Files')) return;
    dragDepth += 1;
  }
  function onDragLeave() { dragDepth = Math.max(0, dragDepth - 1); }
  function onDrop(e) {
    dragDepth = 0;
    if (!e.dataTransfer) return;
    e.preventDefault();

    // Read the entries synchronously — the DataTransfer is neutered the moment
    // this handler yields, so anything awaited first comes back empty. A
    // dropped *folder* only exists in this list; dataTransfer.files flattens it
    // to a single zero-byte entry with no contents.
    const entries = readDropEntries(e.dataTransfer);
    if (entries) uploadDropTree(entries);
    else if (e.dataTransfer.files?.length) uploadFiles(e.dataTransfer.files);
  }

  /** Recreate a dropped directory tree, then upload its files into it. */
  async function uploadDropTree(entries) {
    let tree;
    try {
      tree = await buildDropTree(entries);
    } catch (e) {
      toast(e?.message ?? 'Could not read the dropped items.', 'error');
      return;
    }
    if (!tree.length) return;

    const total = countTreeFiles(tree);
    if (total === 0 && !tree.some((n) => n.kind === 'dir')) return;

    try {
      await uploadInto(tree, $currentFolderId);
    } catch (e) {
      uploadError = e?.message ?? 'Upload failed.';
      toast(uploadError, 'error');
    } finally {
      await reconcile();
    }
  }

  /**
   * Depth-first: create each folder before uploading anything into it, so a
   * child never references a parent that does not exist yet.
   */
  async function uploadInto(nodes, parentId) {
    const siblings = new Set(
      ($childrenByParent.get(parentId ?? null) ?? []).map((n) => n.name),
    );

    for (const node of nodes) {
      const name = uniqueName(node.name, siblings);
      siblings.add(name);

      if (node.kind === 'dir') {
        const created = await createFolder($jmapAccountId, $jmapSession, name, parentId);
        if (!created?.id) {
          // Children have to be created against a known parent id, so an
          // unconfirmed folder stops this branch rather than silently
          // flattening its contents into the parent.
          throw new Error(`Could not confirm the folder "${name}" — its contents were not uploaded.`);
        }
        fileNodes.update((list) => [...list, created]);
        if (node.children?.length) await uploadInto(node.children, created.id);
      } else {
        await uploadOne(node.file, name, parentId);
      }
    }
  }

  /** Upload a single file with its own progress row. @returns {boolean} ok */
  async function uploadOne(file, name, parentId) {
    const id = ++uploadSeq;
    uploads.update((u) => [...u, { id, name, progress: 0, error: '', waiting: '', done: false }]);
    const patch = (fields) =>
      uploads.update((u) => u.map((x) => (x.id === id ? { ...x, ...fields } : x)));

    try {
      const created = await uploadFile($jmapAccountId, $jmapSession, file, {
        parentId,
        name,
        onProgress: (p) => patch({ progress: p, waiting: '' }),
        onWait: (ms) => patch({ waiting: `rate limited — retrying in ${Math.ceil(ms / 1000)}s` }),
      });
      // Null means the server did not echo the new node. reconcile() reads the
      // real state back, so nothing needs inventing here.
      if (created) fileNodes.update((list) => [...list, created]);
      patch({ progress: 1, done: true });
      setTimeout(() => uploads.update((u) => u.filter((x) => x.id !== id)), 1200);
      return true;
    } catch (e) {
      patch({ error: e?.message ?? 'Upload failed.', done: true });
      // A toast is easy to miss on a phone, so the failure also stays pinned in
      // the upload list until dismissed.
      uploadError = e?.message ?? 'Upload failed.';
      toast(`${name}: ${e?.message ?? 'upload failed'}`, 'error');
      return false;
    }
  }

  // ── Keyboard ──────────────────────────────────────────────────────────────

  function onKeydown(e) {
    if ($previewNode) return;
    const tag = e.target?.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA' || e.target?.isContentEditable) return;
    if (e.metaKey || e.ctrlKey) {
      if (e.key === 'a') { e.preventDefault(); selectAll(); }
      return;
    }
    if (e.key === 'Escape') { clearSelection(); confirmingDelete = false; }
    if ((e.key === 'Delete' || e.key === 'Backspace') && selectionCount) {
      e.preventDefault();
      confirmingDelete = true;
    }
  }

  // ── Lifecycle ─────────────────────────────────────────────────────────────

  onMount(async () => {
    const config = await getAppConfig();
    stalwartUrl = config.stalwartUrl ?? '';

    if (!$jmapAccountId) {
      try {
        const session = await getJMAPSession();
        if (!session) return;
        jmapSession.set(session);
        const accountId = Object.keys(session.accounts ?? {})[0];
        jmapAccountId.set(accountId);
        currentUser.set(session.username || session.accounts?.[accountId]?.name || '');
      } catch (e) {
        filesError.set(e?.message ?? 'Could not reach the server.');
        return;
      }
    }

    supported = supportsFiles($jmapSession);
    if (supported) await loadNodes();
  });

  onDestroy(() => previewNode.set(null));

  const toolbarBtn =
    'inline-flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium rounded-lg ' +
    'text-gray-600 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-700 ' +
    'disabled:opacity-40 disabled:cursor-not-allowed transition-colors duration-150';
</script>

<svelte:window on:keydown={onKeydown} />

<div class="flex flex-col app-shell bg-gray-100 dark:bg-gray-950">
  <Navbar {stalwartUrl} />

  <div class="flex flex-1 min-h-0 overflow-hidden">

    <!-- ── Sidebar: folder tree ─────────────────────────────────────────── -->
    <SidebarDrawer width={$sidebarWidth}>
      <div class="flex-1 overflow-y-auto px-2 py-2">
        <div class="px-1 pt-2 pb-1 flex items-center justify-between">
          <span class="text-xs font-semibold text-gray-400 dark:text-gray-500 uppercase tracking-wider select-none">
            Locations
          </span>
        </div>

        <!-- Root -->
        <button
          on:click={() => openFolder(null)}
          on:dragover|preventDefault
          on:drop|preventDefault={(e) => {
            const id = e.dataTransfer?.getData('text/jmap-filenode');
            if (id) moveInto(id, null);
          }}
          class="w-full flex items-center gap-2 px-3 py-2 rounded-lg text-sm transition-colors duration-150
                 {$currentFolderId === null && !searching
                   ? 'bg-blue-100 dark:bg-blue-900/50 text-blue-700 dark:text-blue-300 font-medium'
                   : 'text-gray-700 dark:text-gray-300 hover:bg-gray-200 dark:hover:bg-gray-800'}"
        >
          <FileIcon node={{ blobId: null }} size="sm" />
          <span class="truncate">Files</span>
        </button>

        <!-- Folder tree. Recursive, so it lives in its own component:
             <svelte:self> in a route would refer to the page, not the subtree. -->
        <FolderTree parentId={null} onMove={moveInto} {busyId} />

        {#if $filesLoading && $fileNodes.length === 0}
          {#each Array(4) as _}
            <div class="flex items-center gap-2 px-3 py-2">
              <div class="w-4 h-4 rounded bg-gray-200 dark:bg-gray-700 animate-pulse"></div>
              <div class="h-3 flex-1 max-w-[6rem] rounded bg-gray-200 dark:bg-gray-700 animate-pulse"></div>
            </div>
          {/each}
        {/if}
      </div>

      <!-- Storage summary -->
      <div class="px-4 py-3 border-t border-gray-200 dark:border-gray-700 flex-shrink-0">
        <p class="text-xs text-gray-500 dark:text-gray-400">
          {$fileNodes.length} item{$fileNodes.length === 1 ? '' : 's'} · {formatBytes($totalUsage)}
        </p>
      </div>

    </SidebarDrawer>

    <!-- ── Main pane ────────────────────────────────────────────────────── -->
    <!-- svelte-ignore a11y-no-static-element-interactions -->
    <div
      class="flex-1 min-w-0 h-full flex flex-col bg-white dark:bg-gray-900 relative"
      on:dragenter={onDragEnter}
      on:dragover|preventDefault
      on:dragleave={onDragLeave}
      on:drop={onDrop}
    >
      <!-- Breadcrumbs + toolbar -->
      <div class="flex items-center gap-2 px-4 py-2.5 border-b border-gray-200 dark:border-gray-700 flex-shrink-0">
        <nav class="flex items-center gap-1 min-w-0 flex-1 text-sm" aria-label="Breadcrumb">
          <button on:click={() => openFolder(null)}
            class="px-1.5 py-0.5 rounded text-gray-600 dark:text-gray-300
                   hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors duration-100">
            Files
          </button>
          {#each trail as crumb (crumb.id)}
            <span class="text-gray-300 dark:text-gray-600 select-none">/</span>
            <button on:click={() => openFolder(crumb.id)}
              class="px-1.5 py-0.5 rounded truncate max-w-[12rem]
                     text-gray-600 dark:text-gray-300
                     hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors duration-100">
              {crumb.name}
            </button>
          {/each}
          {#if searching}
            <span class="text-gray-300 dark:text-gray-600 select-none">/</span>
            <span class="px-1.5 py-0.5 text-gray-500 dark:text-gray-400 italic">
              Search results
            </span>
          {/if}
        </nav>

        <!-- Search -->
        <div class="relative flex items-center flex-shrink-0">
          <svg class="absolute left-2.5 w-3.5 h-3.5 text-gray-400 pointer-events-none"
               viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <circle cx="11" cy="11" r="8" /><path d="m21 21-4.35-4.35" />
          </svg>
          <input
            value={$fileSearch}
            on:input={(e) => fileSearch.set(e.currentTarget.value)}
            placeholder="Search files…"
            class="w-44 text-xs pl-8 pr-2 py-1.5 rounded-lg
                   bg-gray-100 dark:bg-gray-700/60 text-gray-800 dark:text-gray-200
                   placeholder-gray-400 dark:placeholder-gray-500
                   border border-transparent focus:border-blue-400 dark:focus:border-blue-500
                   focus:outline-none transition-colors duration-150"
          />
        </div>
      </div>

      <!-- Action bar -->
      <div class="flex items-center gap-1 px-3 py-2 border-b border-gray-200 dark:border-gray-700 flex-shrink-0">
        {#if selectionCount > 0}
          <span class="text-xs font-medium text-gray-600 dark:text-gray-300 px-2">
            {selectionCount} selected
          </span>
          <button class={toolbarBtn} on:click={clearSelection}>Clear</button>
          <div class="w-px h-4 bg-gray-200 dark:bg-gray-600 mx-1"></div>
          {#if confirmingDelete}
            <span class="text-xs text-gray-600 dark:text-gray-300 px-1">Delete permanently?</span>
            <button
              on:click={removeSelected}
              disabled={deleting}
              class="inline-flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium rounded-lg
                     bg-red-500 hover:bg-red-600 text-white disabled:opacity-60
                     transition-colors duration-150"
            >
              {#if deleting}
                <Spinner size="xs" label="" accent="border-t-white" cls="border-white/40" />
                Deleting…
              {:else}
                Delete
              {/if}
            </button>
            <button class={toolbarBtn} on:click={() => (confirmingDelete = false)}>Cancel</button>
          {:else}
            <button class="{toolbarBtn} text-red-600 dark:text-red-400 hover:bg-red-50 dark:hover:bg-red-900/20"
                    on:click={() => (confirmingDelete = true)}>
              <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                   stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
                <path d="M4 7h16M9 7V5h6v2M6 7l1 13h10l1-13" />
              </svg>
              Delete
            </button>
          {/if}
        {:else}
          <button class={toolbarBtn} on:click={() => fileInput?.click()}>
            <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 19V7m0 0-4 4m4-4 4 4M5 5h14" />
            </svg>
            Upload
          </button>
          <button class={toolbarBtn}
                  on:click={() => { creatingFolder = true; newFolderName = ''; }}>
            <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
              <path d="M3 7a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
              <path d="M12 11v5M9.5 13.5h5" />
            </svg>
            New Folder
          </button>
        {/if}

        <div class="flex-1"></div>

        <!-- Sort -->
        <select
          value={$sortKey}
          on:change={(e) => sortKey.set(e.currentTarget.value)}
          aria-label="Sort by"
          class="text-xs rounded-md border border-gray-200 dark:border-gray-600
                 bg-white dark:bg-gray-700 text-gray-600 dark:text-gray-300
                 px-1.5 py-1 focus:outline-none focus:ring-1 focus:ring-blue-500"
        >
          <option value="name">Name</option>
          <option value="size">Size</option>
          <option value="modified">Date</option>
        </select>
        <button class={toolbarBtn} on:click={() => sortAsc.update((v) => !v)}
                title={$sortAsc ? 'Ascending' : 'Descending'}>
          <svg class="w-3.5 h-3.5 transition-transform duration-150"
               style="transform: rotate({$sortAsc ? '0deg' : '180deg'})"
               viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75"
               stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 5v14m0-14 5 5m-5-5-5 5" />
          </svg>
        </button>

        <div class="w-px h-4 bg-gray-200 dark:bg-gray-600 mx-1"></div>

        <!-- View toggle -->
        <button class={toolbarBtn} on:click={() => viewMode.set('grid')}
                title="Grid view" aria-pressed={$viewMode === 'grid'}>
          <svg class="w-3.5 h-3.5 {$viewMode === 'grid' ? 'text-blue-500' : ''}"
               viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75">
            <rect x="3" y="3" width="7" height="7" rx="1" /><rect x="14" y="3" width="7" height="7" rx="1" />
            <rect x="3" y="14" width="7" height="7" rx="1" /><rect x="14" y="14" width="7" height="7" rx="1" />
          </svg>
        </button>
        <button class={toolbarBtn} on:click={() => viewMode.set('list')}
                title="List view" aria-pressed={$viewMode === 'list'}>
          <svg class="w-3.5 h-3.5 {$viewMode === 'list' ? 'text-blue-500' : ''}"
               viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75"
               stroke-linecap="round">
            <path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01" />
          </svg>
        </button>
      </div>

      <input bind:this={fileInput} type="file" multiple class="hidden" on:change={onFilePicked} />

      <!-- New folder inline row -->
      {#if creatingFolder}
        <div class="flex items-center gap-2 px-4 py-2 border-b border-gray-200 dark:border-gray-700
                    bg-blue-50/50 dark:bg-blue-900/10" transition:fly={{ y: -4, duration: 120 }}>
          <FileIcon node={{ blobId: null }} size="sm" />
          <!-- svelte-ignore a11y-autofocus -->
          <input
            bind:value={newFolderName}
            autofocus
            placeholder="Folder name"
            on:keydown={(e) => {
              if (e.key === 'Enter') submitNewFolder();
              if (e.key === 'Escape') { creatingFolder = false; newFolderName = ''; }
            }}
            class="flex-1 max-w-xs text-sm bg-transparent border-b border-blue-400
                   text-gray-800 dark:text-gray-100 focus:outline-none py-0.5"
          />
          <button class="text-xs text-blue-600 dark:text-blue-400 font-medium"
                  on:click={submitNewFolder}>Create</button>
          <button class="text-xs text-gray-400"
                  on:click={() => { creatingFolder = false; newFolderName = ''; }}>Cancel</button>
        </div>
      {/if}

      <!-- Write failures stay on screen: a toast is far too easy to miss on a
           phone, and a silently dropped upload is the worst outcome here. -->
      {#if uploadError}
        <div class="flex items-start gap-2 px-4 py-2.5 flex-shrink-0
                    bg-red-50 dark:bg-red-900/25
                    border-b border-red-200 dark:border-red-800/60">
          <svg class="w-4 h-4 mt-0.5 flex-shrink-0 text-red-500" viewBox="0 0 24 24" fill="none"
               stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="9" /><path d="M12 8v5M12 16.5h.01" />
          </svg>
          <span class="text-xs text-red-800 dark:text-red-200 flex-1 min-w-0">{uploadError}</span>
          <button on:click={() => (uploadError = '')} aria-label="Dismiss"
            class="text-xs text-red-600 dark:text-red-300 hover:underline flex-shrink-0">
            Dismiss
          </button>
        </div>
      {/if}

      <!-- Upload progress -->
      {#if $uploads.length}
        <div class="px-4 py-2 border-b border-gray-200 dark:border-gray-700 space-y-1.5 flex-shrink-0">
          {#each $uploads as up (up.id)}
            <div class="flex items-center gap-2" transition:fade={{ duration: 120 }}>
              {#if up.error}
                <svg class="w-3.5 h-3.5 text-red-500 flex-shrink-0" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2" stroke-linecap="round">
                  <circle cx="12" cy="12" r="9" /><path d="M12 8v5M12 16h.01" />
                </svg>
              {:else if up.done}
                <svg class="w-3.5 h-3.5 text-green-500 flex-shrink-0" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                  <path d="m20 6-11 11-5-5" />
                </svg>
              {:else}
                <Spinner size="xs" label="" />
              {/if}
              <span class="text-xs text-gray-600 dark:text-gray-300 truncate flex-1 min-w-0">{up.name}</span>
              {#if up.error}
                <span class="text-xs text-red-500 truncate max-w-[16rem]">{up.error}</span>
              {:else if up.waiting}
                <span class="text-xs text-amber-600 dark:text-amber-400 truncate max-w-[16rem]">
                  {up.waiting}
                </span>
              {:else}
                <div class="w-32 h-1 rounded-full bg-gray-200 dark:bg-gray-700 overflow-hidden flex-shrink-0">
                  <div class="h-full bg-blue-500 transition-all duration-150"
                       style="width: {Math.round(up.progress * 100)}%"></div>
                </div>
                <span class="text-xs text-gray-400 w-9 text-right tabular-nums flex-shrink-0">
                  {Math.round(up.progress * 100)}%
                </span>
              {/if}
            </div>
          {/each}
        </div>
      {/if}

      <!-- Listing -->
      <div class="flex-1 overflow-y-auto">
        {#if !supported}
          <div class="flex flex-col items-center justify-center h-full gap-3 text-gray-400 dark:text-gray-500">
            <svg class="w-10 h-10 opacity-50" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126zM12 15.75h.007v.008H12z" />
            </svg>
            <p class="text-sm">This server does not advertise JMAP file storage.</p>
            <p class="text-xs opacity-70">Expected capability: urn:ietf:params:jmap:filenode</p>
          </div>
        {:else if $filesError}
          <div class="flex flex-col items-center justify-center h-full gap-3">
            <p class="text-sm text-gray-500 dark:text-gray-400">{$filesError}</p>
            <button on:click={loadNodes}
              class="text-xs font-medium px-3 py-1.5 rounded-lg bg-gray-100 dark:bg-gray-700
                     hover:bg-gray-200 dark:hover:bg-gray-600 text-gray-700 dark:text-gray-200
                     transition-colors duration-150">Try again</button>
          </div>
        {:else if $filesLoading && $fileNodes.length === 0}
          <div class="grid grid-cols-[repeat(auto-fill,minmax(9rem,1fr))] gap-3 p-4">
            {#each Array(12) as _}
              <div class="flex flex-col items-center gap-2 p-3 rounded-xl">
                <div class="w-12 h-12 rounded-lg bg-gray-200 dark:bg-gray-700 animate-pulse"></div>
                <div class="h-3 w-3/4 rounded bg-gray-200 dark:bg-gray-700 animate-pulse"></div>
              </div>
            {/each}
          </div>
        {:else if $visibleNodes.length === 0}
          <div class="flex flex-col items-center justify-center h-full gap-2 text-gray-400 dark:text-gray-500">
            <svg class="w-12 h-12 opacity-40" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round">
              <path d="M3 7a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
            </svg>
            <p class="text-sm">{searching ? 'No files match that search' : 'This folder is empty'}</p>
            {#if !searching}
              <p class="text-xs">Drag files here, or use Upload</p>
            {/if}
          </div>

        {:else if $viewMode === 'grid'}
          <div class="grid grid-cols-[repeat(auto-fill,minmax(9rem,1fr))] gap-2 p-4">
            {#each $visibleNodes as node (node.id)}
              {@const selected = $selectedFileIds.has(node.id)}
              {@const isFolder = fileKind(node) === 'folder'}
              <div
                role="button" tabindex="0"
                draggable="true"
                on:dragstart={(e) => e.dataTransfer?.setData('text/jmap-filenode', node.id)}
                on:dragover={(e) => { if (isFolder) e.preventDefault(); }}
                on:drop|preventDefault|stopPropagation={(e) => {
                  const id = e.dataTransfer?.getData('text/jmap-filenode');
                  if (isFolder && id) moveInto(id, node.id);
                }}
                on:dblclick={() => openNode(node)}
                on:click={(e) => (e.metaKey || e.ctrlKey ? toggleSelect(node.id, e) : openNode(node))}
                on:keydown={(e) => { if (e.key === 'Enter') openNode(node); }}
                class="group relative flex flex-col items-center gap-2 p-3 rounded-xl cursor-pointer
                       transition-colors duration-100 text-center
                       {selected ? 'bg-blue-100 dark:bg-blue-900/40'
                                 : 'hover:bg-gray-100 dark:hover:bg-gray-800'}"
              >
                <button
                  on:click={(e) => toggleSelect(node.id, e)}
                  aria-label={selected ? 'Deselect' : 'Select'}
                  class="absolute top-1.5 left-1.5 w-4 h-4 rounded border-2 flex items-center justify-center
                         transition-opacity duration-100
                         {selected ? 'bg-blue-500 border-blue-500 opacity-100'
                                   : 'border-gray-300 dark:border-gray-500 bg-white/80 dark:bg-gray-800/80 opacity-0 group-hover:opacity-100'}"
                >
                  {#if selected}
                    <svg class="w-2.5 h-2.5 text-white" viewBox="0 0 24 24" fill="none"
                         stroke="currentColor" stroke-width="3.5" stroke-linecap="round" stroke-linejoin="round">
                      <path d="m20 6-11 11-5-5" />
                    </svg>
                  {/if}
                </button>

                {#if busyId === node.id}
                  <div class="w-12 h-12 flex items-center justify-center"><Spinner size="md" label="" /></div>
                {:else}
                  <FileIcon {node} size="lg" />
                {/if}

                {#if renamingId === node.id}
                  <!-- svelte-ignore a11y-autofocus -->
                  <input
                    bind:value={renameValue}
                    autofocus
                    on:click|stopPropagation
                    on:blur={() => commitRename(node)}
                    on:keydown|stopPropagation={(e) => {
                      if (e.key === 'Enter') commitRename(node);
                      if (e.key === 'Escape') renamingId = null;
                    }}
                    class="w-full text-xs text-center bg-white dark:bg-gray-700 rounded
                           border border-blue-400 px-1 py-0.5 focus:outline-none
                           text-gray-800 dark:text-gray-100"
                  />
                {:else}
                  <span class="text-xs text-gray-700 dark:text-gray-200 break-words line-clamp-2 w-full">
                    {node.name}
                  </span>
                  <span class="text-[11px] text-gray-400 dark:text-gray-500">
                    {isFolder ? '' : formatBytes(node.size)}
                  </span>
                {/if}

                <button
                  on:click|stopPropagation={() => startRename(node)}
                  aria-label="Rename"
                  class="absolute top-1.5 right-1.5 p-1 rounded opacity-0 group-hover:opacity-100
                         text-gray-400 hover:text-gray-600 dark:hover:text-gray-200
                         hover:bg-gray-200 dark:hover:bg-gray-700 transition-opacity duration-100"
                >
                  <svg class="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                       stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M12 20h9M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4z" />
                  </svg>
                </button>
              </div>
            {/each}
          </div>

        {:else}
          <table class="w-full text-sm">
            <thead class="sticky top-0 bg-white dark:bg-gray-900 z-10">
              <tr class="border-b border-gray-200 dark:border-gray-700 text-left">
                <th class="w-10 px-3 py-2"></th>
                <th class="px-2 py-2 text-xs font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wider">Name</th>
                <th class="px-2 py-2 text-xs font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wider w-28">Size</th>
                <th class="px-2 py-2 text-xs font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wider w-44">Modified</th>
                <th class="w-20 px-2 py-2"></th>
              </tr>
            </thead>
            <tbody>
              {#each $visibleNodes as node (node.id)}
                {@const selected = $selectedFileIds.has(node.id)}
                {@const isFolder = fileKind(node) === 'folder'}
                <tr
                  draggable="true"
                  on:dragstart={(e) => e.dataTransfer?.setData('text/jmap-filenode', node.id)}
                  on:dragover={(e) => { if (isFolder) e.preventDefault(); }}
                  on:drop|preventDefault|stopPropagation={(e) => {
                    const id = e.dataTransfer?.getData('text/jmap-filenode');
                    if (isFolder && id) moveInto(id, node.id);
                  }}
                  on:dblclick={() => openNode(node)}
                  class="group border-b border-gray-100 dark:border-gray-800 cursor-pointer
                         transition-colors duration-100
                         {selected ? 'bg-blue-50 dark:bg-blue-900/30'
                                   : 'hover:bg-gray-50 dark:hover:bg-gray-800/50'}"
                >
                  <td class="px-3 py-2">
                    <button
                      on:click={(e) => toggleSelect(node.id, e)}
                      aria-label={selected ? 'Deselect' : 'Select'}
                      class="w-4 h-4 rounded border-2 flex items-center justify-center
                             {selected ? 'bg-blue-500 border-blue-500'
                                       : 'border-gray-300 dark:border-gray-500'}"
                    >
                      {#if selected}
                        <svg class="w-2.5 h-2.5 text-white" viewBox="0 0 24 24" fill="none"
                             stroke="currentColor" stroke-width="3.5" stroke-linecap="round" stroke-linejoin="round">
                          <path d="m20 6-11 11-5-5" />
                        </svg>
                      {/if}
                    </button>
                  </td>
                  <td class="px-2 py-2">
                    <button class="flex items-center gap-2 min-w-0 w-full text-left"
                            on:click={() => openNode(node)}>
                      {#if busyId === node.id}
                        <Spinner size="sm" label="" />
                      {:else}
                        <FileIcon {node} size="sm" />
                      {/if}
                      {#if renamingId === node.id}
                        <!-- svelte-ignore a11y-autofocus -->
                        <input
                          bind:value={renameValue}
                          autofocus
                          on:click|stopPropagation
                          on:blur={() => commitRename(node)}
                          on:keydown|stopPropagation={(e) => {
                            if (e.key === 'Enter') commitRename(node);
                            if (e.key === 'Escape') renamingId = null;
                          }}
                          class="flex-1 min-w-0 text-sm bg-white dark:bg-gray-700 rounded
                                 border border-blue-400 px-1 py-0.5 focus:outline-none
                                 text-gray-800 dark:text-gray-100"
                        />
                      {:else}
                        <span class="truncate text-gray-800 dark:text-gray-200">{node.name}</span>
                      {/if}
                    </button>
                  </td>
                  <td class="px-2 py-2 text-xs text-gray-500 dark:text-gray-400 tabular-nums">
                    {isFolder ? '—' : formatBytes(node.size)}
                  </td>
                  <td class="px-2 py-2 text-xs text-gray-500 dark:text-gray-400">
                    {node.modified ? new Date(node.modified).toLocaleString() : '—'}
                  </td>
                  <td class="px-2 py-2">
                    <div class="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity duration-100">
                      <button on:click|stopPropagation={() => startRename(node)} aria-label="Rename"
                        class="p-1 rounded text-gray-400 hover:text-gray-600 dark:hover:text-gray-200
                               hover:bg-gray-200 dark:hover:bg-gray-700">
                        <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                             stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
                          <path d="M12 20h9M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4z" />
                        </svg>
                      </button>
                      {#if !isFolder}
                        <button on:click|stopPropagation={() => downloadNode(node)} aria-label="Download"
                          class="p-1 rounded text-gray-400 hover:text-gray-600 dark:hover:text-gray-200
                                 hover:bg-gray-200 dark:hover:bg-gray-700">
                          <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                               stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M12 3v12m0 0-4-4m4 4 4-4M5 19h14" />
                          </svg>
                        </button>
                      {/if}
                    </div>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
      </div>

      <!-- Drag-and-drop overlay -->
      {#if dragDepth > 0}
        <div class="absolute inset-0 z-20 flex items-center justify-center pointer-events-none
                    bg-blue-500/10 border-2 border-dashed border-blue-400 dark:border-blue-500 m-2 rounded-xl"
             transition:fade={{ duration: 100 }}>
          <div class="flex flex-col items-center gap-2 text-blue-600 dark:text-blue-300">
            <svg class="w-10 h-10" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 19V7m0 0-4 4m4-4 4 4M5 5h14" />
            </svg>
            <p class="text-sm font-medium">
              Drop to upload to {trail.length ? trail[trail.length - 1].name : 'Files'}
            </p>
          </div>
        </div>
      {/if}
    </div>
  </div>
</div>

<FilePreview />
<Toasts />
