<script>
  import { onMount, onDestroy, tick } from 'svelte';
  import { fade } from 'svelte/transition';
  import Navbar from '$lib/components/Navbar.svelte';
  import Toasts from '$lib/components/Toasts.svelte';
  import Spinner from '$lib/components/Spinner.svelte';
  import MarkdownToolbar from '$lib/components/MarkdownToolbar.svelte';
  import { jmapAccountId, jmapSession, currentUser, sidebarWidth } from '$lib/stores/mail.js';
  import { getJMAPSession, getAppConfig } from '$lib/api.js';
  import { supportsFiles } from '$lib/files.js';
  import {
    loadNotes, ensureNotesFolder, readNote, writeNote, createNote,
    renameNote, deleteNote, resolvePath, NOTES_FOLDER,
  } from '$lib/notes.js';
  import { renderMarkdown, imageResolver, noteTitle, noteName } from '$lib/markdown.js';
  import { blobUrl } from '$lib/files.js';
  import {
    noteNodes, allFileNodes, notesFolder, notesLoading, notesError, selectedNoteId,
    selectedNote, noteSearch, noteCache, noteView, saveState, saveError, visibleNotes,
    cacheNote, forgetNote,
  } from '$lib/stores/notes.js';
  import { isCompact } from '$lib/stores/viewport.js';
  import { listPaneClass, detailPaneClass } from '$lib/layout.js';
  import { toast } from '$lib/stores/toast.js';

  let stalwartUrl = '';
  let supported   = true;
  let draft       = '';        // the editor's working copy
  let loadedId    = null;      // which note `draft` belongs to
  let loadingBody = false;
  let creating    = false;
  let deleting    = false;
  let confirmingDelete = false;
  let renaming    = false;
  let renameValue = '';
  let textarea;
  let toolbar;

  const AUTOSAVE_MS = 1200;
  let saveTimer;

  /*
   * Relative image links are resolved against the folder the note lives in, so
   * an attachments folder beside the note works the way its author intended.
   */
  $: resolveImage = imageResolver((path) => {
    const node = resolvePath($allFileNodes, $selectedNote?.parentId ?? null, path, {
      rootFolderId: $notesFolder?.id ?? null,
    });
    return node ? blobUrl(node, { inline: true }) : null;
  });

  $: rendered = renderMarkdown(draft, { resolveImage });
  $: currentTitle = $selectedNote ? noteTitle($selectedNote.name, draft) : '';

  // ── Loading ───────────────────────────────────────────────────────────────

  async function refresh() {
    if (!$jmapAccountId) return;
    notesLoading.set(true);
    notesError.set('');
    try {
      const { nodes, folder, notes } = await loadNotes($jmapAccountId, $jmapSession);
      allFileNodes.set(nodes);
      notesFolder.set(folder);
      noteNodes.set(notes);
      // Pre-load the bodies so search and previews work across the collection
      // rather than only for notes that happen to have been opened.
      primeCache(notes);
    } catch (e) {
      notesError.set(e?.message ?? 'Could not load your notes.');
    } finally {
      notesLoading.set(false);
    }
  }

  /**
   * Fetch note bodies in the background, a few at a time.
   *
   * Serial would be slow on a large collection and all-at-once would trip the
   * per-minute download limit, so this walks the list in small batches.
   */
  async function primeCache(notes, batchSize = 6) {
    const pending = notes.filter((n) => !$noteCache.has(n.id));
    for (let i = 0; i < pending.length; i += batchSize) {
      const batch = pending.slice(i, i + batchSize);
      await Promise.all(batch.map(async (node) => {
        try {
          cacheNote(node.id, await readNote(node));
        } catch {
          // A body that will not load should not break the list; the note is
          // still there and still openable, which will surface the error then.
        }
      }));
    }
  }

  async function openNote(node) {
    if (!node || node.id === loadedId) return;
    await flushSave();                       // never lose the outgoing note
    selectedNoteId.set(node.id);

    const cached = $noteCache.get(node.id);
    if (cached !== undefined) {
      draft = cached;
      loadedId = node.id;
      saveState.set('idle');
      return;
    }

    loadingBody = true;
    try {
      const text = await readNote(node);
      if ($selectedNoteId !== node.id) return;   // superseded
      cacheNote(node.id, text);
      draft = text;
      loadedId = node.id;
      saveState.set('idle');
    } catch (e) {
      notesError.set(e?.message ?? 'Could not open this note.');
    } finally {
      loadingBody = false;
    }
  }

  // ── Saving ────────────────────────────────────────────────────────────────

  function onInput() {
    saveState.set('dirty');
    clearTimeout(saveTimer);
    saveTimer = setTimeout(save, AUTOSAVE_MS);
  }

  async function save() {
    clearTimeout(saveTimer);
    const node = $selectedNote;
    if (!node || $saveState === 'saving') return;
    if (($noteCache.get(node.id) ?? '') === draft) { saveState.set('idle'); return; }

    const text = draft;
    saveState.set('saving');
    saveError.set('');
    try {
      const updated = await writeNote($jmapAccountId, $jmapSession, node, text);
      cacheNote(node.id, text);
      noteNodes.update((list) => list.map((n) => (n.id === node.id ? { ...n, ...updated } : n)));
      saveState.set('saved');
      setTimeout(() => saveState.update((s) => (s === 'saved' ? 'idle' : s)), 1500);
    } catch (e) {
      saveState.set('error');
      saveError.set(e?.message ?? 'Could not save this note.');
    }
  }

  /** Save immediately if there are unsaved edits — used before navigating away. */
  async function flushSave() {
    clearTimeout(saveTimer);
    if ($saveState === 'dirty') await save();
  }

  // Leaving the page or closing the tab must not silently drop an edit.
  function onBeforeUnload(event) {
    if ($saveState === 'dirty' || $saveState === 'saving') {
      save();
      event.preventDefault();
      event.returnValue = '';
    }
  }

  // ── Mutations ─────────────────────────────────────────────────────────────

  async function newNote() {
    if (creating) return;
    creating = true;
    try {
      let folder = $notesFolder;
      if (!folder) {
        const { folder: created } = await ensureNotesFolder($jmapAccountId, $jmapSession, []);
        folder = created;
        notesFolder.set(folder);
      }
      const existing = $noteNodes.map((n) => n.name);
      const { node, text } = await createNote($jmapAccountId, $jmapSession, folder.id, {
        title: 'Untitled', existingNames: existing,
      });
      noteNodes.update((list) => [{ ...node, folderPath: '' }, ...list]);
      cacheNote(node.id, text);
      selectedNoteId.set(node.id);
      draft = text;
      loadedId = node.id;
      saveState.set('idle');
      noteView.set('edit');
      await tick();
      textarea?.focus();
    } catch (e) {
      toast(e?.message ?? 'Could not create the note.', 'error');
    } finally {
      creating = false;
    }
  }

  function startRename() {
    if (!$selectedNote) return;
    renameValue = noteName($selectedNote.name);
    renaming = true;
  }

  async function commitRename() {
    const node = $selectedNote;
    const value = renameValue.trim();
    renaming = false;
    if (!node || !value || value === noteName(node.name)) return;
    try {
      await renameNote($jmapAccountId, $jmapSession, node.id, value);
      const name = /\.(md|markdown|mdown|mkd)$/i.test(value) ? value : `${value}.md`;
      noteNodes.update((list) => list.map((n) => (n.id === node.id ? { ...n, name } : n)));
      toast('Note renamed', 'success');
    } catch (e) {
      toast(e?.message ?? 'Rename failed.', 'error');
    }
  }

  async function removeNote() {
    const node = $selectedNote;
    if (!node || deleting) return;
    deleting = true;
    try {
      await deleteNote($jmapAccountId, $jmapSession, node.id);
      noteNodes.update((list) => list.filter((n) => n.id !== node.id));
      forgetNote(node.id);
      selectedNoteId.set(null);
      draft = '';
      loadedId = null;
      toast('Note deleted', 'success');
    } catch (e) {
      toast(e?.message ?? 'Delete failed.', 'error');
    } finally {
      deleting = false;
      confirmingDelete = false;
    }
  }

  // ── Keyboard ──────────────────────────────────────────────────────────────

  function onKeydown(event) {
    // Only while the editor has focus — Ctrl/Cmd+B anywhere else is not ours.
    if (document.activeElement === textarea && toolbar?.handleShortcut(event)) return;
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') {
      event.preventDefault();
      save();
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
        notesError.set(e?.message ?? 'Could not reach the server.');
        return;
      }
    }

    supported = supportsFiles($jmapSession);
    if (supported) await refresh();
  });

  onDestroy(() => {
    clearTimeout(saveTimer);
    // A pending edit must not be lost by navigating to another app.
    if (typeof window !== 'undefined') flushSave();
  });

  const toolBtn =
    'inline-flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium rounded-lg ' +
    'text-gray-600 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-700 ' +
    'disabled:opacity-40 disabled:cursor-not-allowed transition-colors duration-150';
</script>

<svelte:window on:keydown={onKeydown} on:beforeunload={onBeforeUnload} />

<div class="flex flex-col app-shell bg-gray-100 dark:bg-gray-950">
  <Navbar {stalwartUrl} />

  <div class="flex flex-1 min-h-0 overflow-hidden">

    <!-- ── Note list ─────────────────────────────────────────────────────── -->
    <!--
      The note list is this app's primary content, not a sidebar. Putting it in
      the off-canvas drawer meant that on a phone the list was hidden behind the
      hamburger *and* the editor was hidden for want of a selection — so the
      page rendered completely blank until a note was chosen from elsewhere.
      It now behaves like the message list in Mail: visible by default, replaced
      by the editor on drill-down.
    -->
    <aside
      class="h-full flex flex-col bg-white dark:bg-gray-900
             border-r border-gray-200 dark:border-gray-700
             {listPaneClass($isCompact, !!$selectedNoteId)}"
      style={$isCompact ? '' : `width: ${$sidebarWidth}px`}
    >
      <div class="px-3 py-2.5 border-b border-gray-200 dark:border-gray-700 flex-shrink-0
                  flex items-center justify-between gap-2">
        <h2 class="text-xs font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wider">
          Notes
        </h2>
        <button on:click={newNote} disabled={creating || !supported} title="New note"
          class="w-7 h-7 flex items-center justify-center rounded-md
                 text-gray-400 hover:text-gray-600 dark:hover:text-gray-300
                 hover:bg-gray-100 dark:hover:bg-gray-700
                 disabled:opacity-40 transition-colors duration-150">
          {#if creating}
            <Spinner size="xs" label="Creating" />
          {:else}
            <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 5v14M5 12h14" />
            </svg>
          {/if}
        </button>
      </div>

      <div class="px-3 py-2 border-b border-gray-200 dark:border-gray-700 flex-shrink-0">
        <div class="relative flex items-center">
          <svg class="absolute left-2.5 w-3.5 h-3.5 text-gray-400 pointer-events-none"
               viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <circle cx="11" cy="11" r="8" /><path d="m21 21-4.35-4.35" />
          </svg>
          <input
            value={$noteSearch}
            on:input={(e) => noteSearch.set(e.currentTarget.value)}
            placeholder="Search notes…"
            class="w-full text-xs pl-8 pr-2 py-1.5 rounded-lg
                   bg-gray-100 dark:bg-gray-700/60 text-gray-800 dark:text-gray-200
                   placeholder-gray-400 dark:placeholder-gray-500
                   border border-transparent focus:border-blue-400 dark:focus:border-blue-500
                   focus:outline-none transition-colors duration-150"
          />
        </div>
      </div>

      <div class="flex-1 overflow-y-auto">
        {#if $notesLoading && $noteNodes.length === 0}
          {#each Array(6) as _}
            <div class="px-4 py-3 border-b border-gray-100 dark:border-gray-800 space-y-2">
              <div class="h-3 w-2/3 rounded bg-gray-200 dark:bg-gray-700 animate-pulse"></div>
              <div class="h-2.5 w-full rounded bg-gray-100 dark:bg-gray-700/60 animate-pulse"></div>
            </div>
          {/each}
        {:else if $visibleNotes.length === 0}
          <div class="flex flex-col items-center justify-center h-full gap-2 px-6 text-center
                      text-gray-400 dark:text-gray-500">
            <svg class="w-10 h-10 opacity-40" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round">
              <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
              <path d="M14 3v5h5M9 13h6M9 17h4" />
            </svg>
            <p class="text-sm">
              {$noteSearch.trim() ? 'No notes match that search' : 'No notes yet'}
            </p>
            {#if !$noteSearch.trim() && !$notesFolder}
              <p class="text-xs">A “{NOTES_FOLDER}” folder will be created on your first note.</p>
            {/if}
          </div>
        {:else}
          {#each $visibleNotes as item (item.node.id)}
            {@const active = $selectedNoteId === item.node.id}
            <button
              on:click={() => openNote(item.node)}
              class="w-full text-left px-4 py-3 border-b border-gray-100 dark:border-gray-800
                     transition-colors duration-100
                     {active ? 'bg-blue-50 dark:bg-blue-900/30'
                             : 'hover:bg-gray-50 dark:hover:bg-gray-800/50'}"
            >
              <div class="flex items-baseline justify-between gap-2">
                <span class="text-sm font-medium truncate
                             {active ? 'text-blue-700 dark:text-blue-300'
                                     : 'text-gray-800 dark:text-gray-100'}">
                  {item.title}
                </span>
                <span class="text-[11px] text-gray-400 dark:text-gray-500 flex-shrink-0">
                  {item.node.modified ? new Date(item.node.modified).toLocaleDateString() : ''}
                </span>
              </div>
              {#if item.preview}
                <p class="text-xs text-gray-500 dark:text-gray-400 truncate mt-0.5">{item.preview}</p>
              {/if}
              {#if item.folderPath}
                <p class="text-[11px] text-gray-400 dark:text-gray-600 truncate mt-0.5">
                  {item.folderPath}
                </p>
              {/if}
            </button>
          {/each}
        {/if}
      </div>
    </aside>

    <!-- ── Editor / preview ──────────────────────────────────────────────── -->
    <div class="h-full flex flex-col bg-white dark:bg-gray-900
                {detailPaneClass($isCompact, !!$selectedNoteId)}">

      {#if !supported}
        <div class="flex flex-col items-center justify-center h-full gap-3 px-6 text-center
                    text-gray-400 dark:text-gray-500">
          <p class="text-sm">This server does not advertise JMAP file storage.</p>
          <p class="text-xs opacity-70">Notes are stored as Markdown files, so it is required.</p>
        </div>

      {:else if $notesError}
        <div class="flex flex-col items-center justify-center h-full gap-3">
          <p class="text-sm text-gray-500 dark:text-gray-400">{$notesError}</p>
          <button on:click={refresh} class={toolBtn}>Try again</button>
        </div>

      {:else if !$selectedNote}
        <div class="flex flex-col items-center justify-center h-full gap-3 px-6 text-center
                    text-gray-400 dark:text-gray-500">
          <svg class="w-12 h-12 opacity-30" viewBox="0 0 24 24" fill="none" stroke="currentColor"
               stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 20h9M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4z" />
          </svg>
          <p class="text-sm">Select a note, or create one</p>
        </div>

      {:else}
        <!-- Header -->
        <div class="px-4 py-2.5 border-b border-gray-200 dark:border-gray-700 flex-shrink-0
                    flex items-center gap-2">
          {#if $isCompact}
            <button on:click={() => { flushSave(); selectedNoteId.set(null); loadedId = null; }}
              aria-label="Back to notes"
              class="w-8 h-8 -ml-1 flex items-center justify-center rounded-lg flex-shrink-0
                     text-blue-600 dark:text-blue-400
                     hover:bg-blue-50 dark:hover:bg-blue-900/30 transition-colors duration-150">
              <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                   stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="m15 18-6-6 6-6" />
              </svg>
            </button>
          {/if}

          {#if renaming}
            <!-- svelte-ignore a11y-autofocus -->
            <input
              bind:value={renameValue}
              autofocus
              on:blur={commitRename}
              on:keydown={(e) => {
                if (e.key === 'Enter') commitRename();
                if (e.key === 'Escape') renaming = false;
              }}
              class="flex-1 min-w-0 text-sm font-semibold bg-transparent
                     border-b border-blue-400 text-gray-900 dark:text-gray-100
                     focus:outline-none py-0.5"
            />
          {:else}
            <button on:click={startRename} title="Rename note"
              class="flex-1 min-w-0 text-left text-sm font-semibold truncate
                     text-gray-900 dark:text-gray-100 hover:text-blue-600 dark:hover:text-blue-400
                     transition-colors duration-150">
              {currentTitle}
            </button>
          {/if}

          <!-- Save state -->
          <span class="text-[11px] flex-shrink-0 flex items-center gap-1.5
                       {$saveState === 'error' ? 'text-red-500' : 'text-gray-400 dark:text-gray-500'}">
            {#if $saveState === 'saving'}
              <Spinner size="xs" label="" /> Saving…
            {:else if $saveState === 'saved'}
              <svg class="w-3.5 h-3.5 text-green-500" viewBox="0 0 24 24" fill="none"
                   stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="m20 6-11 11-5-5" />
              </svg>
              Saved
            {:else if $saveState === 'dirty'}
              Unsaved
            {:else if $saveState === 'error'}
              {$saveError || 'Save failed'}
            {/if}
          </span>

          <!-- View mode -->
          <div class="flex items-center rounded-lg bg-gray-100 dark:bg-gray-800 p-0.5 flex-shrink-0">
            {#each [['preview', 'Read'], ['edit', 'Write'], ['split', 'Both']] as [mode, label]}
              <button
                on:click={() => noteView.set(mode)}
                class="px-2 py-1 text-[11px] font-medium rounded-md transition-colors duration-150
                       {mode === 'split' ? 'hidden lg:block' : ''}
                       {$noteView === mode
                         ? 'bg-white dark:bg-gray-700 text-gray-800 dark:text-gray-100 shadow-sm'
                         : 'text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-200'}"
              >{label}</button>
            {/each}
          </div>

          {#if confirmingDelete}
            <button on:click={removeNote} disabled={deleting}
              class="inline-flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium rounded-lg
                     bg-red-500 hover:bg-red-600 text-white disabled:opacity-60 flex-shrink-0">
              {#if deleting}<Spinner size="xs" label="" accent="border-t-white" cls="border-white/40" />{/if}
              Delete
            </button>
            <button on:click={() => (confirmingDelete = false)} class={toolBtn}>Cancel</button>
          {:else}
            <button on:click={() => (confirmingDelete = true)} title="Delete note"
              class="w-8 h-8 flex items-center justify-center rounded-lg flex-shrink-0
                     text-gray-400 hover:text-red-500 hover:bg-red-50 dark:hover:bg-red-900/20
                     transition-colors duration-150">
              <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                   stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
                <path d="M4 7h16M9 7V5h6v2M6 7l1 13h10l1-13" />
              </svg>
            </button>
          {/if}
        </div>

        <!-- Body -->
        {#if loadingBody}
          <div class="flex-1 flex items-center justify-center gap-2 text-sm text-gray-400">
            <Spinner size="sm" label="" /> Loading note…
          </div>
        {:else}
          <div class="flex-1 min-h-0 flex flex-col" in:fade={{ duration: 120 }}>
            {#if $noteView === 'edit' || $noteView === 'split'}
              <MarkdownToolbar bind:this={toolbar} bind:value={draft} {textarea} />
            {/if}

            <div class="flex-1 min-h-0 flex">
            {#if $noteView === 'edit' || $noteView === 'split'}
              <textarea
                bind:this={textarea}
                bind:value={draft}
                on:input={onInput}
                on:blur={save}
                spellcheck="true"
                placeholder="Write in Markdown…"
                class="flex-1 min-w-0 h-full resize-none p-5 font-mono text-sm leading-relaxed
                       bg-transparent text-gray-800 dark:text-gray-100
                       placeholder-gray-400 dark:placeholder-gray-600
                       focus:outline-none
                       {$noteView === 'split' ? 'border-r border-gray-200 dark:border-gray-700' : ''}"
              ></textarea>
            {/if}

            {#if $noteView === 'preview' || $noteView === 'split'}
              <div class="flex-1 min-w-0 h-full overflow-y-auto p-5">
                <!-- Sanitised in renderMarkdown(); see markdown.js for why this
                     is the only boundary here, unlike the mail iframe. -->
                <article class="note-prose">{@html rendered}</article>
              </div>
            {/if}
            </div>
          </div>
        {/if}
      {/if}
    </div>
  </div>
</div>

<Toasts />
