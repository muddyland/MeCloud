<script>
  import { isTyping } from '$lib/dom.js';
  import { onMount, onDestroy } from 'svelte';
  import {
    emails, selectedEmailId, selectedMailbox, loading,
    draggedEmailId, contextMenu, composeOpen, jmapAccountId, searchQuery,
    selectedEmailIds, movePickerOpen, mailboxes, visibleEmails, mailRefresher, currentUser
  } from '$lib/stores/mail.js';
  import { getEmails, searchEmails, bulkMarkSeen, getThreadEmails } from '$lib/api.js';
  import { deleteMessages } from '$lib/mailActions.js';
  import {
    appendUnique, groupByThread, groupIds, conversationMessages,
    navigationOrder, shownInConversation, cardSummary,
  } from '$lib/mailList.js';
  import { toast } from '$lib/stores/toast.js';
  import Avatar from './Avatar.svelte';
  import Spinner from './Spinner.svelte';

  const PAGE = 50;

  let loadingMore  = false;
  let hasMore      = true;
  let sentinel;
  let observer;
  let searchInput;
  let bulkLoading  = false;
  let bulkAction   = '';     // which bulk button is working, for its spinner
  let refreshing   = false;

  // Search state
  let searchResults  = [];
  let searchLoading  = false;
  let searchHasMore  = true;
  let searchPosition = 0;
  let debounceTimer;

  $: isSearching     = $searchQuery.trim().length > 0;
  $: displayedEmails = isSearching ? searchResults : $emails;
  $: selectionMode   = $selectedEmailIds.size > 0;

  // ── Conversations ─────────────────────────────────────────────────────────
  //
  // One card per thread, showing its newest message. Expanding a card loads
  // the whole thread — your replies from Sent included — and lists each
  // message indented beneath it. Actions on the card act on every message the
  // card holds in this list; actions on an indented message act on that one.

  let expanded = new Set();          // group keys
  let threads  = new Map();          // group key -> { size, list }
  const inFlight = new Set();

  $: groups = groupByThread(displayedEmails);

  $: hiddenMailboxIds = new Set($mailboxes.filter((m) => m.role === 'trash' || m.role === 'junk').map((m) => m.id));
  $: mailboxNames     = new Map($mailboxes.map((m) => [m.id, m.name]));
  $: currentMailboxId = isSearching ? null : ($selectedMailbox?.id ?? null);
  $: keep = (m) => shownInConversation(m, currentMailboxId, hiddenMailboxIds);

  // Publish what's on screen so j/k navigation walks the same list the user
  // sees: one stop per collapsed conversation, every message of an open one.
  $: visibleEmails.set(navigationOrder(groups, expanded, threads, keep));

  // An open conversation that gained or lost a message reloads its thread.
  $: for (const g of groups) {
    const cached = threads.get(g.key);
    if (expanded.has(g.key) && cached && cached.size !== g.size) loadThread(g);
  }

  async function loadThread(group) {
    if (!group.threadId || !$jmapAccountId || inFlight.has(group.key)) return;
    inFlight.add(group.key);
    const size = group.size;
    try {
      const list = await getThreadEmails($jmapAccountId, group.threadId);
      threads = new Map(threads).set(group.key, { size, list });
    } catch {
      // The card still expands to the messages this list already holds.
      threads = new Map(threads).set(group.key, { size, list: null });
    } finally {
      inFlight.delete(group.key);
    }
  }

  function toggleExpand(group, e) {
    e?.stopPropagation();
    const next = new Set(expanded);
    if (next.has(group.key)) {
      next.delete(group.key);
    } else {
      next.add(group.key);
      if (threads.get(group.key)?.size !== group.size) loadThread(group);
    }
    expanded = next;
  }

  function openGroup(group) {
    selectedEmailId.set(group.latest.id);
    if (cardSummary(group, keep, $currentUser).count > 1 && !expanded.has(group.key)) toggleExpand(group);
  }

  // The ids a drag or right-click carries: the whole selection when the row is
  // part of it, otherwise just what the row stands for.
  function idsFor(ids) {
    return ids.every((id) => $selectedEmailIds.has(id)) && $selectedEmailIds.size > ids.length
      ? [...$selectedEmailIds]
      : ids;
  }

  function folderLabel(m) {
    const ids = Object.keys(m?.mailboxIds ?? {});
    if (!ids.length || (currentMailboxId && ids.includes(currentMailboxId))) return '';
    return mailboxNames.get(ids.find((id) => !hiddenMailboxIds.has(id)) ?? ids[0]) ?? '';
  }

  // One skeleton block covers both the first mailbox load and the first page of
  // a search; only the row count differs.
  $: showSkeleton  = isSearching
    ? (searchLoading && searchResults.length === 0)
    : $loading;
  $: skeletonRows  = isSearching ? 6 : 10;

  async function refresh() {
    const run = $mailRefresher;
    if (!run || refreshing) return;
    refreshing = true;
    try {
      await run();
    } finally {
      refreshing = false;
    }
  }

  // Reset pagination, clear search and selection when mailbox changes
  let prevMailboxId = null;
  $: {
    const id = $selectedMailbox?.id ?? null;
    if (id !== prevMailboxId) {
      prevMailboxId = id;
      hasMore       = true;
      loadingMore   = false;
      expanded      = new Set();
      threads       = new Map();
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
        searchResults   = appendUnique(searchResults, more);
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
      if (more.length > 0) emails.update(list => appendUnique(list, more));
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
    if (e.key === '/' && !isTyping(document.activeElement)) {
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
  });

  onDestroy(() => {
    observer?.disconnect();
    clearTimeout(debounceTimer);
    visibleEmails.set([]);
    if (typeof window !== 'undefined') {
      window.removeEventListener('keydown', handleGlobalKeydown);
    }
  });

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

  // Select or deselect a whole conversation card at once.
  function toggleSelect(ids, e) {
    e?.stopPropagation();
    selectedEmailIds.update(s => {
      const next = new Set(s);
      const all = ids.every((id) => next.has(id));
      for (const id of ids) { if (all) next.delete(id); else next.add(id); }
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
    bulkAction  = seen ? 'read' : 'unread';
    // A Set here rather than Array.includes inside the map below: selecting a
    // few hundred messages made that pairing quadratic and visibly janky.
    const idSet = new Set($selectedEmailIds);
    const ids   = [...idSet];

    let unreadDelta = 0;
    for (const e of $emails) {
      if (!idSet.has(e.id)) continue;
      const wasSeen = !!e.keywords?.['$seen'];
      if (seen && !wasSeen) unreadDelta--;
      if (!seen && wasSeen) unreadDelta++;
    }

    try {
      await bulkMarkSeen($jmapAccountId, ids, seen);
      emails.update(list => list.map(e =>
        idSet.has(e.id)
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
      bulkAction  = '';
    }
  }

  async function handleBulkDelete() {
    if (bulkLoading) return;
    bulkLoading = true;
    bulkAction  = 'delete';
    const ids   = [...$selectedEmailIds];
    try {
      await deleteMessages(ids);
      clearSelection();
    } catch (e) {
      toast(e?.message ?? 'Delete failed', 'error');
    } finally {
      bulkLoading = false;
      bulkAction  = '';
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
    <div class="px-3 py-2.5 border-b border-gray-200 dark:border-gray-700 shrink-0 flex items-center gap-1">
      <!-- Select-all / clear toggle -->
      <button
        on:click={allSelected ? clearSelection : selectAll}
        title={allSelected ? 'Deselect all' : 'Select all'}
        class="shrink-0 w-7 h-7 flex items-center justify-center rounded
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
        class="shrink-0 w-7 h-7 flex items-center justify-center rounded
               text-gray-400 dark:text-gray-500
               hover:text-gray-600 dark:hover:text-gray-300
               hover:bg-gray-100 dark:hover:bg-gray-700
               disabled:opacity-40 disabled:cursor-not-allowed
               transition-colors duration-150"
      >
        {#if bulkAction === 'read'}
          <Spinner size="xs" label="Marking as read" />
        {:else}
          <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="3"/><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7z"/>
          </svg>
        {/if}
      </button>

      <!-- Mark unread -->
      <button
        on:click={() => bulkMark(false)}
        disabled={bulkLoading}
        title="Mark as unread"
        class="shrink-0 w-7 h-7 flex items-center justify-center rounded
               text-gray-400 dark:text-gray-500
               hover:text-gray-600 dark:hover:text-gray-300
               hover:bg-gray-100 dark:hover:bg-gray-700
               disabled:opacity-40 disabled:cursor-not-allowed
               transition-colors duration-150"
      >
        {#if bulkAction === 'unread'}
          <Spinner size="xs" label="Marking as unread" />
        {:else}
          <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="3"/><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7z"/>
            <line x1="3" y1="3" x2="21" y2="21"/>
          </svg>
        {/if}
      </button>

      <!-- Move -->
      <button
        on:click={handleBulkMove}
        disabled={bulkLoading}
        title="Move to folder"
        class="shrink-0 w-7 h-7 flex items-center justify-center rounded
               text-gray-400 dark:text-gray-500
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
        class="shrink-0 w-7 h-7 flex items-center justify-center rounded
               text-red-400 dark:text-red-500
               hover:text-red-600 dark:hover:text-red-400
               hover:bg-red-50 dark:hover:bg-red-900/20
               disabled:opacity-40 disabled:cursor-not-allowed
               transition-colors duration-150"
      >
        {#if bulkAction === 'delete'}
          <Spinner size="xs" label="Deleting" accent="border-t-red-500" />
        {:else}
          <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 01-2 2H8a2 2 0 01-2-2L5 6"/>
            <path d="M10 11v6M14 11v6"/><path d="M9 6V4a1 1 0 011-1h4a1 1 0 011 1v2"/>
          </svg>
        {/if}
      </button>

      <!-- Clear selection -->
      <button
        on:click={clearSelection}
        title="Clear selection"
        class="shrink-0 p-1.5 rounded text-gray-400 dark:text-gray-500
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
    <div class="px-4 py-3 border-b border-gray-200 dark:border-gray-700 shrink-0 flex items-center justify-between gap-2">
      <h2 class="text-xs font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wider truncate">
        {isSearching ? 'Search results' : ($selectedMailbox?.name ?? 'Inbox')}
      </h2>
      <div class="flex items-center gap-0.5 shrink-0">
        <button
          on:click={refresh}
          disabled={refreshing || !$mailRefresher}
          title="Refresh ( . )"
          class="w-7 h-7 flex items-center justify-center rounded-md
                 text-gray-400 dark:text-gray-500
                 hover:text-gray-600 dark:hover:text-gray-300
                 hover:bg-gray-100 dark:hover:bg-gray-700
                 disabled:opacity-40 disabled:cursor-not-allowed
                 transition-colors duration-150"
        >
          {#if refreshing}
            <Spinner size="xs" label="Refreshing" />
          {:else}
            <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
              <path d="M21 12a9 9 0 11-2.64-6.36M21 3v6h-6"/>
            </svg>
          {/if}
        </button>
        <button
          on:click={() => composeOpen.set(true)}
          title="Compose new message ( c )"
          class="w-7 h-7 flex items-center justify-center rounded-md
                 text-gray-400 dark:text-gray-500
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
    </div>
  {/if}

  <!-- Search bar -->
  <div class="px-3 py-2 border-b border-gray-200 dark:border-gray-700 shrink-0">
    <div class="relative flex items-center">
      <svg class="absolute left-2.5 w-3.5 h-3.5 text-gray-400 dark:text-gray-500 pointer-events-none shrink-0"
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
               focus:outline-hidden focus:ring-0
               transition-colors duration-150"
      />
      {#if searchLoading}
        <span class="absolute right-2.5 flex items-center">
          <Spinner size="xs" label="Searching" />
        </span>
      {:else if $searchQuery}
        <button
          on:click={() => { searchQuery.set(''); searchInput?.focus(); }}
          title="Clear search"
          class="absolute right-2 p-0.5 rounded-sm text-gray-400 hover:text-gray-600 dark:hover:text-gray-200 transition-colors duration-100"
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
    {#if showSkeleton}
      {#each Array(skeletonRows) as _}
        <div class="flex items-center gap-3 px-4 py-3 border-b border-gray-100 dark:border-gray-700/50">
          <div class="w-9 h-9 rounded-full bg-gray-200 dark:bg-gray-700 animate-pulse shrink-0"></div>
          <div class="flex-1 min-w-0 space-y-2">
            <div class="flex justify-between">
              <div class="h-3 bg-gray-200 dark:bg-gray-700 rounded-sm animate-pulse w-1/3"></div>
              <div class="h-3 bg-gray-200 dark:bg-gray-700 rounded-sm animate-pulse w-10"></div>
            </div>
            <div class="h-3 bg-gray-200 dark:bg-gray-700 rounded-sm animate-pulse w-2/3"></div>
            <div class="h-2.5 bg-gray-100 dark:bg-gray-700/60 rounded-sm animate-pulse w-full"></div>
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
      {#each groups as group (group.key)}
        {@const email      = group.latest}
        {@const ids        = groupIds(group)}
        {@const summary    = cardSummary(group, keep, $currentUser)}
        {@const isThread   = summary.count > 1}
        {@const isOpen     = isThread && expanded.has(group.key)}
        {@const convo      = isOpen ? conversationMessages(group, threads.get(group.key)?.list).filter(keep) : []}
        {@const holdsOpen  = ids.includes($selectedEmailId) || convo.some((m) => m.id === $selectedEmailId)}
        {@const selected   = isOpen ? false : holdsOpen}
        {@const checked    = ids.every((id) => $selectedEmailIds.has(id))}
        {@const unread     = group.unread > 0}
        {@const s          = sender(email.from)}
        {@const who        = isThread ? summary.people.join(', ') : (s.name || s.email || 'Unknown')}
        <div
          id={isOpen ? undefined : `email-row-${email.id}`}
          draggable="true"
          on:dragstart={() => {
            const carried = idsFor(ids);
            draggedEmailId.set(carried.length === 1 ? carried[0] : carried);
          }}
          on:dragend={() => draggedEmailId.set(null)}
          on:contextmenu={(e) => {
            e.preventDefault();
            contextMenu.set({ x: e.clientX, y: e.clientY, emailId: email.id, ids: idsFor(ids) });
          }}
          class="flex items-center transition-colors duration-150 group
                 {isOpen ? '' : 'border-b border-gray-100 dark:border-gray-700/50'}
                 {checked || selected
                   ? 'bg-blue-50 dark:bg-blue-900/30'
                   : 'hover:bg-gray-50 dark:hover:bg-gray-700/40'}
                 {(Array.isArray($draggedEmailId) ? ids.some((id) => $draggedEmailId.includes(id)) : ids.includes($draggedEmailId)) ? 'opacity-50' : ''}"
        >
          <!-- Checkbox / Avatar toggle area -->
          <button
            on:click={(e) => toggleSelect(ids, e)}
            title={checked ? 'Deselect' : (isThread ? 'Select conversation' : 'Select')}
            class="shrink-0 flex items-center justify-center w-12 self-stretch
                   focus:outline-hidden focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-blue-400"
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

          <!-- Expand / collapse a conversation. Kept as its own column so the
               text of single messages and conversations lines up. -->
          {#if isThread}
            <button
              on:click={(e) => toggleExpand(group, e)}
              title={isOpen ? 'Collapse conversation' : 'Expand conversation'}
              aria-expanded={isOpen}
              class="shrink-0 w-5 -ml-1 self-stretch flex items-center justify-center
                     text-gray-400 dark:text-gray-500 hover:text-gray-700 dark:hover:text-gray-200
                     focus:outline-hidden focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-blue-400"
            >
              <svg class="w-3.5 h-3.5 transition-transform duration-150 {isOpen ? 'rotate-90' : ''}"
                   viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.25"
                   stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"/></svg>
            </button>
          {:else}
            <div class="shrink-0 w-5 -ml-1" aria-hidden="true"></div>
          {/if}

          <!-- Email content (opens the newest message, and the conversation) -->
          <button
            on:click={() => openGroup(group)}
            class="flex-1 min-w-0 flex items-center gap-2 py-3 pr-4 text-left
                   focus:outline-hidden focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-blue-400"
          >
            <div class="flex-1 min-w-0">
              <div class="flex items-baseline justify-between gap-1 mb-0.5">
                <span class="text-sm truncate
                  {unread
                    ? 'font-semibold text-gray-900 dark:text-gray-50'
                    : 'font-medium text-gray-700 dark:text-gray-300'}">
                  {who}
                </span>
                <span class="text-xs shrink-0 text-gray-400 dark:text-gray-500">
                  {formatDate(email.receivedAt)}
                </span>
              </div>
              <div class="flex items-center gap-1.5 mb-0.5">
                {#if group.emails.some((e) => e.hasAttachment)}
                  <svg class="w-3 h-3 shrink-0 text-gray-400 dark:text-gray-500"
                       viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                       stroke-linecap="round" stroke-linejoin="round" aria-label="Has attachment">
                    <path d="M21.44 11.05l-9.19 9.19a6 6 0 01-8.49-8.49l9.19-9.19a4 4 0 015.66 5.66l-9.2 9.19a2 2 0 01-2.83-2.83l8.49-8.48"/>
                  </svg>
                {/if}
                <span class="text-xs truncate
                  {unread
                    ? 'font-medium text-gray-800 dark:text-gray-200'
                    : 'text-gray-600 dark:text-gray-400'}">
                  {email.subject || '(no subject)'}
                </span>
                {#if isThread}
                  <span class="shrink-0 text-[10px] font-medium px-1.5 rounded-full
                               bg-gray-100 dark:bg-gray-700 text-gray-500 dark:text-gray-400"
                        title="{summary.count} messages in this conversation">{summary.count}</span>
                {/if}
              </div>
              {#if email.preview && !isOpen}
                <div class="text-xs truncate text-gray-400 dark:text-gray-500">
                  {email.preview}
                </div>
              {/if}
            </div>

            {#if unread}
              <div class="w-2 h-2 rounded-full bg-blue-500 shrink-0"
                   aria-label={group.unread > 1 ? `${group.unread} unread` : 'Unread'}></div>
            {/if}
          </button>
        </div>

        <!-- The open conversation: each message indented under its card,
             newest first, with a folder tag on anything from outside this one. -->
        {#if isOpen}
          <div class="pb-1.5 border-b border-gray-100 dark:border-gray-700/50" role="group"
               aria-label="Messages in this conversation">
            {#each convo as m (m.id)}
              {@const mSelected = $selectedEmailId === m.id}
              {@const mUnread   = isUnread(m)}
              {@const ms        = sender(m.from)}
              {@const folder    = folderLabel(m)}
              <button
                id="email-row-{m.id}"
                draggable="true"
                on:dragstart={() => draggedEmailId.set(m.id)}
                on:dragend={() => draggedEmailId.set(null)}
                on:contextmenu={(e) => {
                  e.preventDefault();
                  contextMenu.set({ x: e.clientX, y: e.clientY, emailId: m.id, ids: [m.id] });
                }}
                on:click={() => selectedEmailId.set(m.id)}
                class="w-full flex items-center gap-2 pl-4 pr-4 py-2 text-left ml-[3.75rem] max-w-[calc(100%-3.75rem)]
                       border-l-2 transition-colors duration-150
                       focus:outline-hidden focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-blue-400
                       {mSelected
                         ? 'border-blue-500 bg-blue-50 dark:bg-blue-900/30'
                         : 'border-gray-200 dark:border-gray-700 hover:bg-gray-50 dark:hover:bg-gray-700/40'}"
              >
                <div class="flex-1 min-w-0">
                  <div class="flex items-baseline justify-between gap-1">
                    <span class="text-xs truncate
                      {mUnread ? 'font-semibold text-gray-900 dark:text-gray-50' : 'font-medium text-gray-700 dark:text-gray-300'}">
                      {ms.email && ms.email.toLowerCase() === String($currentUser).toLowerCase() ? 'Me' : (ms.name || ms.email || 'Unknown')}
                    </span>
                    <span class="text-[11px] shrink-0 text-gray-400 dark:text-gray-500">{formatDate(m.receivedAt)}</span>
                  </div>
                  <div class="flex items-center gap-1.5">
                    {#if folder}
                      <span class="shrink-0 text-[10px] px-1.5 rounded-sm bg-gray-100 dark:bg-gray-700
                                   text-gray-500 dark:text-gray-400">{folder}</span>
                    {/if}
                    <span class="text-xs truncate text-gray-400 dark:text-gray-500">{m.preview ?? ''}</span>
                  </div>
                </div>
                {#if mUnread}
                  <div class="w-1.5 h-1.5 rounded-full bg-blue-500 shrink-0" aria-label="Unread"></div>
                {/if}
              </button>
            {/each}
            {#if !threads.has(group.key)}
              <div class="flex items-center gap-2 ml-[3.75rem] pl-4 py-1.5 text-[11px] text-gray-400">
                <Spinner size="xs" label="" /> Loading conversation…
              </div>
            {/if}
          </div>
        {/if}
      {/each}

    {/if}

    <!-- Sentinel always in DOM so onMount can observe it immediately -->
    <div bind:this={sentinel} class="h-8 flex items-center justify-center gap-2">
      {#if loadingMore || (searchLoading && searchResults.length > 0)}
        <Spinner size="sm" label="" />
        <span class="text-xs text-gray-400 dark:text-gray-500">Loading more…</span>
      {/if}
    </div>

    {#if isSearching && !searchHasMore && searchResults.length > 0}
      <p class="text-xs text-center text-gray-400 dark:text-gray-600 py-3">All results loaded</p>
    {:else if !isSearching && !hasMore && $emails.length > 0}
      <p class="text-xs text-center text-gray-400 dark:text-gray-600 py-3">All messages loaded</p>
    {/if}
  </div>
</section>
