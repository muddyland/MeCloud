<script>
  import { onMount, onDestroy } from 'svelte';
  import {
    emails, selectedEmailId, selectedMailbox, loading,
    draggedEmailId, contextMenu, composeOpen, jmapAccountId, searchQuery,
    selectedEmailIds, movePickerOpen, mailboxes
  } from '$lib/stores/mail.js';
  import { getEmails, searchEmails, bulkMarkSeen, bulkDestroy, bulkMove } from '$lib/api.js';
  import { toast } from '$lib/stores/toast.js';
  import Avatar from './Avatar.svelte';

  const PAGE = 50;

  let loadingMore  = false;
  let hasMore      = true;
  let sentinel;
  let observer;
  let searchInput;
  let bulkLoading  = false;

  // Search state
  let searchResults  = [];
  let searchLoading  = false;
  let searchHasMore  = true;
  let searchPosition = 0;
  let debounceTimer;

  $: isSearching     = $searchQuery.trim().length > 0;
  $: displayedEmails = isSearching ? searchResults : $emails;
  $: selectionMode   = $selectedEmailIds.size > 0;

  // Reset pagination, clear search and selection when mailbox changes
  let prevMailboxId = null;
  $: {
    const id = $selectedMailbox?.id ?? null;
    if (id !== prevMailboxId) {
      prevMailboxId = id;
      hasMore       = true;
      loadingMore   = false;
      selectedEmailIds.set(new Set());
      if ($searchQuery) searchQuery.set('');
    }
  }

  // Debounced search triggered by query changes
  $: {
    clearTimeout(debounceTimer);
    const q = $searchQuery.trim();
    if (q) {
      debounceTimer = setTimeout(() => runSearch(q), 350);
    } else {
      searchResults  = [];
      searchHasMore  = true;
      searchPosition = 0;
      searchLoading  = false;
    }
  }

  async function runSearch(q) {
    if (!$jmapAccountId) return;
    searchLoading  = true;
    searchResults  = [];
    searchPosition = 0;
    searchHasMore  = true;
    try {
      const results = await searchEmails($jmapAccountId, q, 0, PAGE);
      if ($searchQuery.trim() !== q) return;
      searchResults  = results;
      searchPosition = results.length;
      if (results.length < PAGE) searchHasMore = false;
    } catch {
      // silent
    } finally {
      searchLoading = false;
    }
  }

  async function loadMoreSearch() {
    const q = $searchQuery.trim();
    if (!q || searchLoading || !searchHasMore || !$jmapAccountId) return;
    searchLoading = true;
    try {
      const more = await searchEmails($jmapAccountId, q, searchPosition, PAGE);
      if ($searchQuery.trim() !== q) return;
      if (more.length < PAGE) searchHasMore = false;
      if (more.length > 0) {
        searchResults   = [...searchResults, ...more];
        searchPosition += more.length;
      }
    } catch {
      // silent
    } finally {
      searchLoading = false;
    }
  }

  async function loadMore() {
    if (isSearching) { await loadMoreSearch(); return; }
    if (loadingMore || !hasMore || $loading || !$jmapAccountId || !$selectedMailbox) return;
    const mailboxId = $selectedMailbox.id;
    const position  = $emails.length;
    loadingMore = true;
    try {
      const more = await getEmails($jmapAccountId, mailboxId, position, PAGE);
      if ($selectedMailbox?.id !== mailboxId) return;
      if (more.length < PAGE) hasMore = false;
      if (more.length > 0) emails.update(list => [...list, ...more]);
    } catch {
      // silent
    } finally {
      loadingMore = false;
    }
  }

  function handleKeydown(e) {
    if (e.key === 'Escape') {
      if ($selectedEmailIds.size > 0) { selectedEmailIds.set(new Set()); return; }
      searchQuery.set(''); searchInput?.blur();
    }
  }

  function handleGlobalKeydown(e) {
    if (e.key === '/' && document.activeElement?.tagName !== 'INPUT' && document.activeElement?.tagName !== 'TEXTAREA') {
      e.preventDefault();
      searchInput?.focus();
    }
  }

  onMount(() => {
    observer = new IntersectionObserver(
      (entries) => { if (entries[0].isIntersecting) loadMore(); },
      { rootMargin: '100px' }
    );
    if (sentinel) observer.observe(sentinel);
    window.addEventListener('keydown', handleGlobalKeydown);
    return () => { observer?.disconnect(); window.removeEventListener('keydown', handleGlobalKeydown); };
  });

  onDestroy(() => { observer?.disconnect(); window.removeEventListener('keydown', handleGlobalKeydown); });

  // ── Helpers ───────────────────────────────────────────────────────────────

  function formatDate(dateStr) {
    if (!dateStr) return '';
    const date = new Date(dateStr);
    const now  = new Date();
    return date.toDateString() === now.toDateString()
      ? date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
      : date.toLocaleDateString([], { month: 'short', day: 'numeric' });
  }

  function sender(from) {
    if (!from?.length) return { name: 'Unknown', email: '' };
    return { name: from[0].name || '', email: from[0].email || '' };
  }

  function isUnread(email) {
    return !email.keywords?.['$seen'];
  }

  // ── Selection ─────────────────────────────────────────────────────────────

  function toggleSelect(id, e) {
    e?.stopPropagation();
    selectedEmailIds.update(s => {
      const next = new Set(s);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  }

  function clearSelection() { selectedEmailIds.set(new Set()); }

  function selectAll() { selectedEmailIds.set(new Set(displayedEmails.map(e => e.id))); }

  $: allSelected = displayedEmails.length > 0 && displayedEmails.every(e => $selectedEmailIds.has(e.id));

  // ── Bulk actions ──────────────────────────────────────────────────────────

  async function bulkMark(seen) {
    if (bulkLoading) return;
    bulkLoading = true;
    const ids = [...$selectedEmailIds];
    let unreadDelta = 0;
    for (const id of ids) {
      const e = $emails.find(e => e.id === id);
      if (!e) continue;
      const wasSeen = !!e.keywords?.['$seen'];
      if (seen && !wasSeen) unreadDelta--;
      if (!seen && wasSeen) unreadDelta++;
    }
    try {
      await bulkMarkSeen($jmapAccountId, ids, seen);
      emails.update(list => list.map(e =>
        ids.includes(e.id)
          ? { ...e, keywords: { ...(e.keywords ?? {}), '$seen': seen ? true : undefined } }
          : e
      ));
      if (unreadDelta !== 0) {
        mailboxes.update(list => list.map(mb =>
          mb.id === $selectedMailbox?.id
            ? { ...mb, unreadEmails: Math.max(0, (mb.unreadEmails ?? 0) + unreadDelta) }
            : mb
        ));
      }
      clearSelection();
    } catch (e) {
      toast(e?.message ?? 'Failed to update messages', 'error');
    } finally {
      bulkLoading = false;
    }
  }

  async function handleBulkDelete() {
    if (bulkLoading) return;
    bulkLoading = true;
    const ids = [...$selectedEmailIds];
    const trashMailbox = $mailboxes.find(m => m.role === 'trash');
    const inTrash = $selectedMailbox?.role === 'trash';
    try {
      if (inTrash) {
        await bulkDestroy($jmapAccountId, ids);
        toast(`${ids.length} message${ids.length === 1 ? '' : 's'} deleted permanently`, 'success');
      } else if (trashMailbox) {
        await bulkMove($jmapAccountId, ids, trashMailbox.id, $selectedMailbox?.id);
        toast(`${ids.length} message${ids.length === 1 ? '' : 's'} moved to Trash`, 'success');
      }
      emails.update(list => list.filter(e => !ids.includes(e.id)));
      if (ids.includes($selectedEmailId)) selectedEmailId.set(null);
      clearSelection();
    } catch (e) {
      toast(e?.message ?? 'Delete failed', 'error');
    } finally {
      bulkLoading = false;
    }
  }

  function handleBulkMove() {
    if ($selectedEmailIds.size === 0) return;
    movePickerOpen.set([...$selectedEmailIds]);
  }
</script>

<section class="flex flex-col h-full w-full bg-white dark:bg-gray-800 border-r border-gray-200 dark:border-gray-700">
  <!-- Header: normal or bulk-action toolbar -->
  {#if selectionMode}
    <div class="px-3 py-2.5 border-b border-gray-200 dark:border-gray-700 flex-shrink-0 flex items-center gap-1">
      <!-- Select-all / clear toggle -->
      <button
        on:click={allSelected ? clearSelection : selectAll}
        title={allSelected ? 'Deselect all' : 'Select all'}
        class="flex-shrink-0 w-7 h-7 flex items-center justify-center rounded
               text-gray-400 dark:text-gray-500
               hover:text-gray-600 dark:hover:text-gray-300
               hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors duration-150"
      >
        {#if allSelected}
          <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="20 6 9 17 4 12"/>
          </svg>
        {:else}
          <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <rect x="3" y="3" width="18" height="18" rx="2"/>
          </svg>
        {/if}
      </button>

      <span class="text-xs font-medium text-gray-600 dark:text-gray-400 flex-1 min-w-0 truncate pl-1">
        {$selectedEmailIds.size} selected
      </span>

      <!-- Mark read -->
      <button
        on:click={() => bulkMark(true)}
        disabled={bulkLoading}
        title="Mark as read"
        class="flex-shrink-0 p-1.5 rounded text-gray-400 dark:text-gray-500
               hover:text-gray-600 dark:hover:text-gray-300
               hover:bg-gray-100 dark:hover:bg-gray-700
               disabled:opacity-40 disabled:cursor-not-allowed
               transition-colors duration-150"
      >
        <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="3"/><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7z"/>
        </svg>
      </button>

      <!-- Mark unread -->
      <button
        on:click={() => bulkMark(false)}
        disabled={bulkLoading}
        title="Mark as unread"
        class="flex-shrink-0 p-1.5 rounded text-gray-400 dark:text-gray-500
               hover:text-gray-600 dark:hover:text-gray-300
               hover:bg-gray-100 dark:hover:bg-gray-700
               disabled:opacity-40 disabled:cursor-not-allowed
               transition-colors duration-150"
      >
        <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="3"/><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7z"/>
          <line x1="3" y1="3" x2="21" y2="21"/>
        </svg>
      </button>

      <!-- Move -->
      <button
        on:click={handleBulkMove}
        disabled={bulkLoading}
        title="Move to folder"
        class="flex-shrink-0 p-1.5 rounded text-gray-400 dark:text-gray-500
               hover:text-gray-600 dark:hover:text-gray-300
               hover:bg-gray-100 dark:hover:bg-gray-700
               disabled:opacity-40 disabled:cursor-not-allowed
               transition-colors duration-150"
      >
        <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
          <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/>
        </svg>
      </button>

      <!-- Delete -->
      <button
        on:click={handleBulkDelete}
        disabled={bulkLoading}
        title={$selectedMailbox?.role === 'trash' ? 'Delete permanently' : 'Delete'}
        class="flex-shrink-0 p-1.5 rounded text-red-400 dark:text-red-500
               hover:text-red-600 dark:hover:text-red-400
               hover:bg-red-50 dark:hover:bg-red-900/20
               disabled:opacity-40 disabled:cursor-not-allowed
               transition-colors duration-150"
      >
        <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 01-2 2H8a2 2 0 01-2-2L5 6"/>
          <path d="M10 11v6M14 11v6"/><path d="M9 6V4a1 1 0 011-1h4a1 1 0 011 1v2"/>
        </svg>
      </button>

      <!-- Clear selection -->
      <button
        on:click={clearSelection}
        title="Clear selection"
        class="flex-shrink-0 p-1.5 rounded text-gray-400 dark:text-gray-500
               hover:text-gray-600 dark:hover:text-gray-300
               hover:bg-gray-100 dark:hover:bg-gray-700
               transition-colors duration-150"
      >
        <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
          <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
        </svg>
      </button>
    </div>
  {:else}
    <div class="px-4 py-3 border-b border-gray-200 dark:border-gray-700 flex-shrink-0 flex items-center justify-between">
      <h2 class="text-xs font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wider">
        {isSearching ? 'Search results' : ($selectedMailbox?.name ?? 'Inbox')}
      </h2>
      <button
        on:click={() => composeOpen.set(true)}
        title="Compose new message"
        class="p-1 rounded-md text-gray-400 dark:text-gray-500
               hover:text-gray-600 dark:hover:text-gray-300
               hover:bg-gray-100 dark:hover:bg-gray-700
               transition-colors duration-150"
      >
        <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
          <path d="M11 4H4a2 2 0 00-2 2v14a2 2 0 002 2h14a2 2 0 002-2v-7"/>
          <path d="M18.5 2.5a2.121 2.121 0 013 3L12 15l-4 1 1-4 9.5-9.5z"/>
        </svg>
      </button>
    </div>
  {/if}

  <!-- Search bar -->
  <div class="px-3 py-2 border-b border-gray-200 dark:border-gray-700 flex-shrink-0">
    <div class="relative flex items-center">
      <svg class="absolute left-2.5 w-3.5 h-3.5 text-gray-400 dark:text-gray-500 pointer-events-none flex-shrink-0"
           viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
           stroke-linecap="round" stroke-linejoin="round">
        <circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/>
      </svg>
      <input
        bind:this={searchInput}
        type="text"
        value={$searchQuery}
        on:input={(e) => searchQuery.set(e.currentTarget.value)}
        on:keydown={handleKeydown}
        placeholder="Search mail… ( / )"
        class="w-full text-xs pl-8 pr-7 py-1.5 rounded-lg
               bg-gray-100 dark:bg-gray-700/60
               text-gray-800 dark:text-gray-200
               placeholder-gray-400 dark:placeholder-gray-500
               border border-transparent focus:border-blue-400 dark:focus:border-blue-500
               focus:outline-none focus:ring-0
               transition-colors duration-150"
      />
      {#if $searchQuery}
        <button
          on:click={() => { searchQuery.set(''); searchInput?.focus(); }}
          title="Clear search"
          class="absolute right-2 p-0.5 rounded text-gray-400 hover:text-gray-600 dark:hover:text-gray-200 transition-colors duration-100"
        >
          <svg class="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"
               stroke-linecap="round" stroke-linejoin="round">
            <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
          </svg>
        </button>
      {/if}
    </div>
  </div>

  <!-- List -->
  <div class="flex-1 overflow-y-auto">
    {#if $loading && !isSearching}
      {#each Array(10) as _}
        <div class="flex items-center gap-3 px-4 py-3 border-b border-gray-100 dark:border-gray-700/50">
          <div class="w-9 h-9 rounded-full bg-gray-200 dark:bg-gray-700 animate-pulse flex-shrink-0"></div>
          <div class="flex-1 min-w-0 space-y-2">
            <div class="flex justify-between">
              <div class="h-3 bg-gray-200 dark:bg-gray-700 rounded animate-pulse w-1/3"></div>
              <div class="h-3 bg-gray-200 dark:bg-gray-700 rounded animate-pulse w-10"></div>
            </div>
            <div class="h-3 bg-gray-200 dark:bg-gray-700 rounded animate-pulse w-2/3"></div>
            <div class="h-2.5 bg-gray-100 dark:bg-gray-700/60 rounded animate-pulse w-full"></div>
          </div>
        </div>
      {/each}
    {:else if searchLoading && searchResults.length === 0}
      {#each Array(6) as _}
        <div class="flex items-center gap-3 px-4 py-3 border-b border-gray-100 dark:border-gray-700/50">
          <div class="w-9 h-9 rounded-full bg-gray-200 dark:bg-gray-700 animate-pulse flex-shrink-0"></div>
          <div class="flex-1 min-w-0 space-y-2">
            <div class="flex justify-between">
              <div class="h-3 bg-gray-200 dark:bg-gray-700 rounded animate-pulse w-1/3"></div>
              <div class="h-3 bg-gray-200 dark:bg-gray-700 rounded animate-pulse w-10"></div>
            </div>
            <div class="h-3 bg-gray-200 dark:bg-gray-700 rounded animate-pulse w-2/3"></div>
            <div class="h-2.5 bg-gray-100 dark:bg-gray-700/60 rounded animate-pulse w-full"></div>
          </div>
        </div>
      {/each}
    {:else if displayedEmails.length === 0}
      <div class="flex flex-col items-center justify-center h-full gap-2 text-gray-400 dark:text-gray-500">
        {#if isSearching}
          <svg class="w-10 h-10 opacity-40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/>
          </svg>
          <span class="text-sm">No results for "{$searchQuery.trim()}"</span>
        {:else}
          <svg class="w-10 h-10 opacity-40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
            <path d="M2.25 13.5h3.86a2.25 2.25 0 012.012 1.244l.256.512a2.25 2.25 0 002.013 1.244h3.218a2.25 2.25 0 002.013-1.244l.256-.512a2.25 2.25 0 012.013-1.244h3.859m-19.5.338V18a2.25 2.25 0 002.25 2.25h15A2.25 2.25 0 0021.75 18v-4.162c0-.224-.034-.447-.1-.661L19.24 5.338a2.25 2.25 0 00-2.15-1.588H6.911a2.25 2.25 0 00-2.15 1.588L2.35 13.177a2.25 2.25 0 00-.1.661z"/>
          </svg>
          <span class="text-sm">No messages</span>
        {/if}
      </div>
    {:else}
      {#each displayedEmails as email (email.id)}
        {@const selected = $selectedEmailId === email.id}
        {@const checked  = $selectedEmailIds.has(email.id)}
        {@const unread   = isUnread(email)}
        {@const s        = sender(email.from)}
        <div
          draggable="true"
          on:dragstart={() => draggedEmailId.set(email.id)}
          on:dragend={() => draggedEmailId.set(null)}
          on:contextmenu={(e) => { e.preventDefault(); contextMenu.set({ x: e.clientX, y: e.clientY, emailId: email.id }); }}
          class="flex items-center border-b border-gray-100 dark:border-gray-700/50
                 transition-colors duration-150 group
                 {checked || selected
                   ? 'bg-blue-50 dark:bg-blue-900/30'
                   : 'hover:bg-gray-50 dark:hover:bg-gray-700/40'}
                 {$draggedEmailId === email.id ? 'opacity-50' : ''}"
        >
          <!-- Checkbox / Avatar toggle area -->
          <button
            on:click={(e) => toggleSelect(email.id, e)}
            title={checked ? 'Deselect' : 'Select'}
            class="flex-shrink-0 flex items-center justify-center w-12 self-stretch
                   focus:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-blue-400"
          >
            <div class="relative w-9 h-9">
              <!-- Avatar: hidden on hover or when checked/selectionMode -->
              <div class="absolute inset-0 flex items-center justify-center transition-opacity duration-100
                          {checked || selectionMode ? 'opacity-0' : 'opacity-100 group-hover:opacity-0'}">
                <Avatar name={s.name} email={s.email} size="md" />
              </div>
              <!-- Checkbox: shown on hover or when checked/selectionMode -->
              <div class="absolute inset-0 flex items-center justify-center transition-opacity duration-100
                          {checked || selectionMode ? 'opacity-100' : 'opacity-0 group-hover:opacity-100'}">
                <div class="w-5 h-5 rounded border-2 flex items-center justify-center transition-colors duration-100
                            {checked
                              ? 'bg-blue-500 border-blue-500'
                              : 'border-gray-300 dark:border-gray-500 bg-white dark:bg-gray-800'}">
                  {#if checked}
                    <svg class="w-3 h-3 text-white" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round">
                      <polyline points="20 6 9 17 4 12"/>
                    </svg>
                  {/if}
                </div>
              </div>
            </div>
          </button>

          <!-- Email content (opens email) -->
          <button
            on:click={() => selectedEmailId.set(email.id)}
            class="flex-1 min-w-0 flex items-center gap-2 py-3 pr-4 text-left
                   focus:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-blue-400"
          >
            <div class="flex-1 min-w-0">
              <div class="flex items-baseline justify-between gap-1 mb-0.5">
                <span class="text-sm truncate
                  {unread
                    ? 'font-semibold text-gray-900 dark:text-gray-50'
                    : 'font-medium text-gray-700 dark:text-gray-300'}">
                  {s.name || s.email || 'Unknown'}
                </span>
                <span class="text-xs flex-shrink-0 text-gray-400 dark:text-gray-500">
                  {formatDate(email.receivedAt)}
                </span>
              </div>
              <div class="text-xs truncate mb-0.5
                {unread
                  ? 'font-medium text-gray-800 dark:text-gray-200'
                  : 'text-gray-600 dark:text-gray-400'}">
                {email.subject || '(no subject)'}
              </div>
              {#if email.preview}
                <div class="text-xs truncate text-gray-400 dark:text-gray-500">
                  {email.preview}
                </div>
              {/if}
            </div>

            {#if unread}
              <div class="w-2 h-2 rounded-full bg-blue-500 flex-shrink-0"></div>
            {/if}
          </button>
        </div>
      {/each}

    {/if}

    <!-- Sentinel always in DOM so onMount can observe it immediately -->
    <div bind:this={sentinel} class="h-4 flex items-center justify-center">
      {#if loadingMore || (searchLoading && searchResults.length > 0)}
        <div class="w-4 h-4 rounded-full border-2 border-gray-300 dark:border-gray-600 border-t-blue-500 animate-spin"></div>
      {/if}
    </div>

    {#if isSearching && !searchHasMore && searchResults.length > 0}
      <p class="text-xs text-center text-gray-400 dark:text-gray-600 py-3">All results loaded</p>
    {:else if !isSearching && !hasMore && $emails.length > 0}
      <p class="text-xs text-center text-gray-400 dark:text-gray-600 py-3">All messages loaded</p>
    {/if}
  </div>
</section>
