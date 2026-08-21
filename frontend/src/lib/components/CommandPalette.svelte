<script>
  import { tick } from 'svelte';
  import { fly, fade } from 'svelte/transition';
  import { goto } from '$app/navigation';
  import {
    commandPaletteOpen, mailboxes, selectedMailbox, composeOpen, composeContext,
    sieveOpen, appPasswordsOpen, shortcutsOpen, newFolderOpen, darkMode, searchQuery,
  } from '$lib/stores/mail.js';
  import { contacts, selectedContact } from '$lib/stores/contacts.js';
  import { calendars, selectedCalendar } from '$lib/stores/calendar.js';
  import { fileNodes, currentFolderId, fileSearch } from '$lib/stores/files.js';
  import { fileKind } from '$lib/fileTypes.js';
  import { rankItems, highlightRuns } from '$lib/fuzzy.js';
  import { logout } from '$lib/api.js';
  import { APPS } from '$lib/apps.js';

  let query = '';
  let cursor = 0;
  let inputEl;
  let listEl;

  function close() {
    commandPaletteOpen.set(false);
    query = '';
    cursor = 0;
  }

  function run(action) {
    close();
    // Let the modal tear down before navigating, so the transition doesn't
    // fight the route change.
    tick().then(action);
  }

  // ── The command set ───────────────────────────────────────────────────────
  //
  // Apps and global actions are always present. Everything else is drawn from
  // whichever stores the current route has populated — the palette surfaces
  // what the app actually knows about rather than refetching.

  $: appItems = APPS.map((app) => ({
    id: `app:${app.id}`,
    label: app.label,
    group: 'Apps',
    keywords: ['go to', 'open', 'switch'],
    boost: 12,          // navigation is the most common intent in a launcher
    icon: 'app',
    run: () => goto(app.href),
  }));

  $: actionItems = [
    { id: 'a:compose', label: 'Compose message', keywords: ['new mail', 'write', 'send'],
      run: () => { composeContext.set(null); composeOpen.set(true); } },
    { id: 'a:newfolder', label: 'New folder', keywords: ['create', 'mailbox'],
      run: () => newFolderOpen.set(true) },
    { id: 'a:rules', label: 'Filters and rules', keywords: ['sieve', 'settings'],
      run: () => sieveOpen.set(true) },
    { id: 'a:apppw', label: 'App passwords', keywords: ['settings', 'security', 'token'],
      run: () => appPasswordsOpen.set(true) },
    { id: 'a:theme', label: $darkMode ? 'Switch to light mode' : 'Switch to dark mode',
      keywords: ['theme', 'dark', 'light', 'appearance'], run: () => darkMode.toggle() },
    { id: 'a:shortcuts', label: 'Keyboard shortcuts', keywords: ['help', 'keys'],
      run: () => shortcutsOpen.set(true) },
    { id: 'a:signout', label: 'Sign out', keywords: ['log out', 'logout', 'exit'],
      run: () => logout() },
  ].map((a) => ({ ...a, group: 'Actions', icon: 'action' }));

  $: mailboxItems = $mailboxes.map((mb) => ({
    id: `mb:${mb.id}`,
    label: mb.name,
    group: 'Mailboxes',
    keywords: ['folder', 'mailbox', mb.role ?? ''].filter(Boolean),
    icon: 'mailbox',
    hint: mb.unreadEmails ? `${mb.unreadEmails} unread` : '',
    run: () => { searchQuery.set(''); selectedMailbox.set(mb); goto('/'); },
  }));

  $: contactItems = $contacts.map((c) => {
    const name = c.name?.full ?? '';
    const email = Object.values(c.emails ?? {})[0]?.address ?? '';
    return {
      id: `c:${c.id}`,
      label: name || email || 'Contact',
      group: 'Contacts',
      keywords: [email].filter(Boolean),
      hint: name ? email : '',
      icon: 'contact',
      run: () => { selectedContact.set(c); goto('/contacts'); },
    };
  });

  $: calendarItems = $calendars.map((cal) => ({
    id: `cal:${cal.id}`,
    label: cal.name,
    group: 'Calendars',
    keywords: ['calendar'],
    icon: 'calendar',
    run: () => { selectedCalendar.set(cal.id); goto('/calendar'); },
  }));

  $: folderItems = $fileNodes
    .filter((n) => fileKind(n) === 'folder')
    .map((n) => ({
      id: `fn:${n.id}`,
      label: n.name,
      group: 'File folders',
      keywords: ['files', 'drive', 'folder'],
      icon: 'folder',
      run: () => { fileSearch.set(''); currentFolderId.set(n.id); goto('/files'); },
    }));

  $: fileItems = $fileNodes
    .filter((n) => fileKind(n) !== 'folder')
    .map((n) => ({
      id: `f:${n.id}`,
      label: n.name,
      group: 'Files',
      keywords: ['file', 'document'],
      icon: 'file',
      run: () => { fileSearch.set(n.name); currentFolderId.set(n.parentId ?? null); goto('/files'); },
    }));

  $: allItems = [
    ...appItems, ...actionItems, ...mailboxItems,
    ...folderItems, ...contactItems, ...calendarItems, ...fileItems,
  ];

  $: results = rankItems(query, allItems, { limit: 40 });

  // Group headings, in the order the groups first appear in the results.
  $: grouped = results.reduce((acc, item) => {
    const bucket = acc.find((g) => g.name === item.group);
    if (bucket) bucket.items.push(item);
    else acc.push({ name: item.group, items: [item] });
    return acc;
  }, []);

  // A flat view of the same order, so arrow keys move through groups naturally.
  $: flat = grouped.flatMap((g) => g.items);
  $: if (cursor >= flat.length) cursor = Math.max(0, flat.length - 1);

  async function open() {
    await tick();
    inputEl?.focus();
  }
  $: if ($commandPaletteOpen) open();

  // Reset the highlight whenever the query changes, or the selection lands on
  // whatever happened to be at the old index.
  $: query, (cursor = 0);

  async function moveCursor(delta) {
    if (!flat.length) return;
    cursor = (cursor + delta + flat.length) % flat.length;
    await tick();
    listEl?.querySelector('[data-active="true"]')?.scrollIntoView({ block: 'nearest' });
  }

  function onKeydown(event) {
    // Open from anywhere. This is deliberately outside the `open` guard.
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
      event.preventDefault();
      commandPaletteOpen.update((v) => !v);
      return;
    }
    if (!$commandPaletteOpen) return;

    if (event.key === 'Escape') { event.preventDefault(); close(); }
    else if (event.key === 'ArrowDown') { event.preventDefault(); moveCursor(1); }
    else if (event.key === 'ArrowUp') { event.preventDefault(); moveCursor(-1); }
    else if (event.key === 'Enter') {
      event.preventDefault();
      const chosen = flat[cursor];
      if (chosen) run(chosen.run);
    }
  }
</script>

<svelte:window on:keydown={onKeydown} />

{#if $commandPaletteOpen}
  <div
    class="fixed inset-0 z-[70] flex items-start justify-center px-4 pt-[12vh]
           bg-gray-900/40 dark:bg-black/60 backdrop-blur-[2px]"
    transition:fade={{ duration: 120 }}
    on:click|self={close}
    role="presentation"
  >
    <div
      transition:fly={{ y: -12, duration: 160 }}
      class="w-full max-w-xl flex flex-col overflow-hidden rounded-2xl
             bg-white dark:bg-gray-800 shadow-2xl shadow-gray-900/25 dark:shadow-black/50
             ring-1 ring-gray-900/5 dark:ring-white/10"
      role="dialog" aria-modal="true" aria-label="Command palette"
    >
      <!-- Query -->
      <div class="flex items-center gap-2.5 px-4 h-12 border-b border-gray-200 dark:border-gray-700">
        <svg class="w-4 h-4 text-gray-400 flex-shrink-0" viewBox="0 0 24 24" fill="none"
             stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="11" cy="11" r="8" /><path d="m21 21-4.35-4.35" />
        </svg>
        <input
          bind:this={inputEl}
          bind:value={query}
          placeholder="Jump to an app, folder, contact or file… or type a command"
          aria-label="Search commands"
          class="flex-1 min-w-0 bg-transparent text-sm text-gray-800 dark:text-gray-100
                 placeholder-gray-400 dark:placeholder-gray-500 outline-none"
        />
        <kbd class="text-[10px] font-sans px-1.5 py-0.5 rounded flex-shrink-0
                    bg-gray-100 dark:bg-gray-700 text-gray-400 dark:text-gray-500">esc</kbd>
      </div>

      <!-- Results -->
      <div bind:this={listEl} class="max-h-[52vh] overflow-y-auto py-1.5">
        {#if flat.length === 0}
          <p class="px-4 py-8 text-center text-sm text-gray-400 dark:text-gray-500">
            Nothing matches “{query}”
          </p>
        {:else}
          {#each grouped as group (group.name)}
            <p class="px-4 pt-2 pb-1 text-[10px] font-semibold uppercase tracking-wider
                      text-gray-400 dark:text-gray-500 select-none">
              {group.name}
            </p>
            {#each group.items as item (item.id)}
              {@const index = flat.indexOf(item)}
              {@const active = index === cursor}
              <button
                data-active={active}
                on:click={() => run(item.run)}
                on:mousemove={() => (cursor = index)}
                class="w-full flex items-center gap-3 px-4 py-2 text-left transition-colors duration-75
                       {active ? 'bg-blue-50 dark:bg-blue-900/40' : ''}"
              >
                <svg class="w-4 h-4 flex-shrink-0 {active ? 'text-blue-500 dark:text-blue-400' : 'text-gray-400 dark:text-gray-500'}"
                     viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75"
                     stroke-linecap="round" stroke-linejoin="round">
                  {#if item.icon === 'app'}
                    <rect x="3" y="3" width="7" height="7" rx="1.5" /><rect x="14" y="3" width="7" height="7" rx="1.5" />
                    <rect x="3" y="14" width="7" height="7" rx="1.5" /><rect x="14" y="14" width="7" height="7" rx="1.5" />
                  {:else if item.icon === 'mailbox'}
                    <path d="M4 4h16v16H4z" /><path d="m4 7 8 6 8-6" />
                  {:else if item.icon === 'contact'}
                    <circle cx="12" cy="8" r="4" /><path d="M4 21v-1a6 6 0 0 1 6-6h4a6 6 0 0 1 6 6v1" />
                  {:else if item.icon === 'calendar'}
                    <rect x="3" y="4" width="18" height="18" rx="2" /><path d="M16 2v4M8 2v4M3 10h18" />
                  {:else if item.icon === 'folder'}
                    <path d="M3 7a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
                  {:else if item.icon === 'file'}
                    <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" /><path d="M14 3v5h5" />
                  {:else}
                    <circle cx="12" cy="12" r="9" /><path d="M12 8v8M8 12h8" />
                  {/if}
                </svg>

                <span class="flex-1 min-w-0 truncate text-sm text-gray-800 dark:text-gray-100">
                  {#each highlightRuns(item.label, item.indices) as part}
                    {#if part.hit}<mark class="bg-transparent text-blue-600 dark:text-blue-400 font-semibold">{part.text}</mark>
                    {:else}{part.text}{/if}
                  {/each}
                </span>

                {#if item.hint}
                  <span class="text-xs text-gray-400 dark:text-gray-500 flex-shrink-0 truncate max-w-[12rem]">
                    {item.hint}
                  </span>
                {/if}
                {#if active}
                  <kbd class="text-[10px] font-sans px-1 py-0.5 rounded flex-shrink-0
                              bg-white dark:bg-gray-700 text-gray-400 dark:text-gray-400
                              border border-gray-200 dark:border-gray-600">↵</kbd>
                {/if}
              </button>
            {/each}
          {/each}
        {/if}
      </div>

      <footer class="flex items-center gap-3 px-4 py-2 border-t border-gray-200 dark:border-gray-700
                     bg-gray-50/60 dark:bg-gray-800/60 text-[11px] text-gray-400 dark:text-gray-500">
        <span>↑↓ navigate</span><span>↵ open</span><span>esc close</span>
        <span class="flex-1"></span>
        <span>{flat.length} result{flat.length === 1 ? '' : 's'}</span>
      </footer>
    </div>
  </div>
{/if}
