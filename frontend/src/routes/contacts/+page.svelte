<script>
  import { onMount } from 'svelte';
  import Navbar from '$lib/components/Navbar.svelte';
  import AppNav from '$lib/components/AppNav.svelte';
  import ContactModal from '$lib/components/ContactModal.svelte';
  import Toasts from '$lib/components/Toasts.svelte';
  import Avatar from '$lib/components/Avatar.svelte';
  import {
    addressBooks, contacts, selectedAddressBook, selectedContact,
    contactsLoading, contactSearch, contactModalOpen, editingContact
  } from '$lib/stores/contacts.js';
  import { jmapAccountId, jmapSession, currentUser, sidebarWidth } from '$lib/stores/mail.js';
  import {
    getAddressBooks, getContacts, getAppConfig, getJMAPSession,
    createAddressBook, deleteAddressBook
  } from '$lib/api.js';
  import { toast } from '$lib/stores/toast.js';

  let stalwartUrl    = '';
  let newBookInput   = false;
  let newBookName    = '';
  let creatingBook   = false;
  let deletingBookId = null;

  async function loadContacts() {
    if (!$jmapAccountId) return;
    contactsLoading.set(true);
    try {
      const list = await getContacts($jmapAccountId, $jmapSession, $selectedAddressBook);
      contacts.set(list);
    } catch (e) {
      console.error('Failed to load contacts', e);
    } finally {
      contactsLoading.set(false);
    }
  }

  $: $selectedAddressBook, $jmapAccountId && loadContacts();

  $: filtered = $contacts.filter(c => {
    const q = $contactSearch.toLowerCase();
    if (!q) return true;
    return fullName(c).toLowerCase().includes(q)
        || allEmails(c).some(e => e.toLowerCase().includes(q));
  }).sort((a, b) => fullName(a).localeCompare(fullName(b)));

  // ── Contact field helpers ────────────────────────────────────────────────────
  function allEmails(c) {
    return Object.values(c.emails ?? {}).map(e => e.address ?? '').filter(Boolean);
  }
  function allPhones(c) {
    return Object.values(c.phones ?? {}).map(p => ({ number: p.number ?? '', type: Object.keys(p.contexts ?? {})[0] ?? '' })).filter(p => p.number);
  }
  function firstEmail(c) { return allEmails(c)[0] ?? ''; }
  function fullName(c)   { return c.name?.full ?? ''; }
  function jobTitle(c)   { return Object.values(c.titles ?? {})[0]?.name ?? ''; }
  function orgName(c)    { return Object.values(c.organizations ?? {})[0]?.name ?? ''; }
  function website(c)    { return Object.values(c.links ?? {})[0]?.href ?? ''; }
  function notes(c)      { return Object.values(c.notes ?? {})[0]?.note ?? ''; }
  function address(c) {
    const a = Object.values(c.addresses ?? {})[0];
    if (!a) return null;
    const comps = Array.isArray(a.components) ? a.components : [];
    const street = comps.find(s => s.kind === 'name')?.value ?? '';
    return { street, city: a.locality ?? '', state: a.region ?? '', postal: a.postcode ?? '', country: a.country ?? '' };
  }
  function birthday(c) {
    const b = Object.values(c.anniversaries ?? {}).find(a => a.kind === 'birth');
    if (!b?.date) return '';
    const { year, month, day } = b.date;
    const parts = [];
    if (month) parts.push(String(month).padStart(2,'0'));
    if (day)   parts.push(String(day).padStart(2,'0'));
    if (year)  parts.push(String(year));
    return parts.join(' / ');
  }

  // ── Address book management ──────────────────────────────────────────────────
  async function confirmNewBook() {
    if (!newBookName.trim()) { newBookInput = false; return; }
    creatingBook = true;
    try {
      const created = await createAddressBook($jmapAccountId, $jmapSession, newBookName.trim());
      if (created) {
        addressBooks.update(list => [...list, { id: created.id, name: newBookName.trim() }]);
        toast('Address book created', 'success');
      }
    } catch (e) { toast(e?.message ?? 'Create failed', 'error'); }
    finally { creatingBook = false; newBookInput = false; newBookName = ''; }
  }

  async function confirmDeleteBook(book) {
    deletingBookId = book.id;
    try {
      await deleteAddressBook($jmapAccountId, $jmapSession, book.id);
      addressBooks.update(list => list.filter(b => b.id !== book.id));
      if ($selectedAddressBook === book.id) selectedAddressBook.set(null);
      toast('Address book deleted', 'success');
    } catch (e) { toast(e?.message ?? 'Delete failed', 'error'); }
    finally { deletingBookId = null; }
  }

  // ── Modal helpers ────────────────────────────────────────────────────────────
  function openNew()  { editingContact.set(null);  contactModalOpen.set(true); }
  function openEdit(c){ editingContact.set(c);     contactModalOpen.set(true); }

  onMount(async () => {
    const config = await getAppConfig();
    stalwartUrl = config.stalwartUrl ?? '';

    if (!$jmapAccountId) {
      try {
        const session = await getJMAPSession();
        jmapSession.set(session);
        const accountId = Object.keys(session.accounts)[0];
        jmapAccountId.set(accountId);
        currentUser.set(session.username || session.accounts[accountId]?.name || '');
      } catch (e) { console.error('JMAP session failed', e); }
    }

    if ($jmapAccountId) {
      try {
        const books = await getAddressBooks($jmapAccountId, $jmapSession);
        addressBooks.set(books);
      } catch (e) { console.error('Failed to load address books', e); }
      await loadContacts();
    }
  });
</script>

<div class="flex flex-col h-screen bg-gray-100 dark:bg-gray-950">
  <Navbar {stalwartUrl} />

  <div class="flex flex-1 min-h-0 overflow-hidden">

    <!-- Left panel -->
    <div class="flex-shrink-0 h-full flex flex-col bg-gray-100 dark:bg-gray-900
                border-r border-gray-200 dark:border-gray-700" style="width: {$sidebarWidth}px">

      <div class="flex-1 overflow-y-auto px-2 py-2">

        <!-- Section heading + new book button -->
        <div class="px-1 pt-2 pb-1 flex items-center justify-between">
          <span class="text-xs font-semibold text-gray-400 dark:text-gray-500 uppercase tracking-wider select-none">
            Address Books
          </span>
          <button on:click={() => { newBookInput = true; newBookName = ''; }}
            title="New address book"
            class="w-5 h-5 flex items-center justify-center rounded text-gray-400 hover:text-gray-600
                   dark:hover:text-gray-300 hover:bg-gray-200 dark:hover:bg-gray-700 transition-colors text-base leading-none">
            +
          </button>
        </div>

        {#if newBookInput}
          <div class="flex items-center gap-1 px-2 py-1">
            <input bind:value={newBookName}
              on:keydown={(e) => { if (e.key === 'Enter') confirmNewBook(); if (e.key === 'Escape') newBookInput = false; }}
              placeholder="Book name"
              class="flex-1 text-xs px-2 py-1 rounded border border-blue-400 dark:border-blue-500
                     bg-white dark:bg-gray-700 text-gray-800 dark:text-gray-100 focus:outline-none"
              autofocus />
            <button on:click={confirmNewBook} disabled={creatingBook}
              class="text-xs text-blue-600 dark:text-blue-400 disabled:opacity-50">✓</button>
            <button on:click={() => newBookInput = false} class="text-xs text-gray-400">✕</button>
          </div>
        {/if}

        <!-- All Contacts -->
        <button on:click={() => selectedAddressBook.set(null)}
          class="w-full flex items-center gap-2 px-3 py-2 rounded-lg text-sm transition-colors duration-150
                 {$selectedAddressBook === null
                   ? 'bg-blue-100 dark:bg-blue-900/50 text-blue-700 dark:text-blue-300 font-medium'
                   : 'text-gray-700 dark:text-gray-300 hover:bg-gray-200 dark:hover:bg-gray-800'}">
          <svg class="w-4 h-4 opacity-60" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75"><path d="M17 21v-2a4 4 0 00-4-4H5a4 4 0 00-4 4v2"/><circle cx="9" cy="7" r="4"/></svg>
          All Contacts
        </button>

        {#each $addressBooks as book}
          <div class="group relative">
            <button on:click={() => selectedAddressBook.set(book.id)}
              class="w-full flex items-center gap-2 px-3 py-2 rounded-lg text-sm transition-colors duration-150
                     {$selectedAddressBook === book.id
                       ? 'bg-blue-100 dark:bg-blue-900/50 text-blue-700 dark:text-blue-300 font-medium'
                       : 'text-gray-700 dark:text-gray-300 hover:bg-gray-200 dark:hover:bg-gray-800'}">
              <svg class="w-4 h-4 opacity-60" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75"><rect x="2" y="3" width="20" height="18" rx="2"/><path d="M8 7h8M8 12h8M8 17h4"/></svg>
              <span class="truncate flex-1 text-left">{book.name}</span>
            </button>
            <button on:click={() => confirmDeleteBook(book)} disabled={deletingBookId === book.id}
              title="Delete address book"
              class="absolute right-1 inset-y-0 hidden group-hover:flex items-center px-1
                     text-gray-400 hover:text-red-500 dark:hover:text-red-400 transition-colors">
              <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
                <path d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 01-2.244 2.077H8.084a2.25 2.25 0 01-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 00-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 013.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 00-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 00-7.5 0"/>
              </svg>
            </button>
          </div>
        {/each}
      </div>

      <AppNav />
    </div>

    <!-- Contact list -->
    <div class="flex-shrink-0 h-full flex flex-col bg-white dark:bg-gray-900
                border-r border-gray-200 dark:border-gray-700 w-72">

      <div class="px-3 py-2 border-b border-gray-200 dark:border-gray-700 flex gap-2">
        <div class="relative flex-1">
          <svg class="absolute left-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-gray-400"
               viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <circle cx="11" cy="11" r="8"/><path d="m21 21-4.35-4.35"/>
          </svg>
          <input
            value={$contactSearch}
            on:input={(e) => contactSearch.set(e.currentTarget.value)}
            placeholder="Search contacts…"
            class="w-full pl-7 pr-3 py-1.5 text-sm rounded-lg border border-gray-200 dark:border-gray-600
                   bg-gray-50 dark:bg-gray-800 text-gray-900 dark:text-gray-100
                   focus:outline-none focus:ring-2 focus:ring-blue-500"
          />
        </div>
        <button on:click={openNew}
          class="flex-shrink-0 px-3 py-1.5 text-sm rounded-lg bg-blue-600 hover:bg-blue-700 text-white transition-colors">
          + New
        </button>
      </div>

      <div class="flex-1 overflow-y-auto">
        {#if $contactsLoading}
          <div class="flex items-center justify-center h-32 text-gray-400 text-sm">Loading…</div>
        {:else if filtered.length === 0}
          <div class="flex items-center justify-center h-32 text-gray-400 text-sm">No contacts</div>
        {:else}
          {#each filtered as contact (contact.id)}
            <button on:click={() => selectedContact.set(contact)}
              class="w-full flex items-center gap-3 px-4 py-3 text-left border-b
                     border-gray-100 dark:border-gray-800 transition-colors duration-100
                     {$selectedContact?.id === contact.id
                       ? 'bg-blue-50 dark:bg-blue-900/30'
                       : 'hover:bg-gray-50 dark:hover:bg-gray-800/50'}">
              <Avatar name={fullName(contact)} email={firstEmail(contact)} size="sm" />
              <div class="min-w-0">
                <div class="text-sm font-medium text-gray-800 dark:text-gray-100 truncate">{fullName(contact) || '—'}</div>
                {#if jobTitle(contact) || orgName(contact)}
                  <div class="text-xs text-gray-500 dark:text-gray-400 truncate">
                    {[jobTitle(contact), orgName(contact)].filter(Boolean).join(' · ')}
                  </div>
                {:else if firstEmail(contact)}
                  <div class="text-xs text-gray-500 dark:text-gray-400 truncate">{firstEmail(contact)}</div>
                {/if}
              </div>
            </button>
          {/each}
        {/if}
      </div>
    </div>

    <!-- Contact detail -->
    <div class="flex-1 min-w-0 h-full bg-white dark:bg-gray-900 overflow-y-auto">
      {#if $selectedContact}
        {@const c = $selectedContact}
        <div class="max-w-xl mx-auto px-8 py-10">

          <!-- Header -->
          <div class="flex items-start justify-between mb-8">
            <div class="flex items-center gap-4">
              <Avatar name={fullName(c)} email={firstEmail(c)} size="lg" />
              <div>
                <h1 class="text-xl font-semibold text-gray-900 dark:text-gray-100">{fullName(c) || '—'}</h1>
                {#if jobTitle(c) || orgName(c)}
                  <p class="text-sm text-gray-500 dark:text-gray-400 mt-0.5">
                    {[jobTitle(c), orgName(c)].filter(Boolean).join(' · ')}
                  </p>
                {/if}
              </div>
            </div>
            <button on:click={() => openEdit(c)}
              class="text-sm px-3 py-1.5 rounded-lg border border-gray-200 dark:border-gray-600
                     text-gray-700 dark:text-gray-300 hover:bg-gray-50 dark:hover:bg-gray-700 transition-colors">
              Edit
            </button>
          </div>

          <dl class="space-y-5">

            <!-- Emails -->
            {#if allEmails(c).length}
              <div>
                <dt class="text-xs font-medium text-gray-400 dark:text-gray-500 uppercase tracking-wider mb-1.5">Email</dt>
                {#each Object.values(c.emails ?? {}) as e}
                  <dd class="flex items-center gap-2 text-sm text-gray-800 dark:text-gray-200 mb-1">
                    {#if Object.keys(e.contexts ?? {})[0]}
                      <span class="text-xs text-gray-400 dark:text-gray-500 w-14 flex-shrink-0 capitalize">
                        {Object.keys(e.contexts)[0]}
                      </span>
                    {/if}
                    <a href="mailto:{e.address}" class="text-blue-600 dark:text-blue-400 hover:underline truncate">{e.address}</a>
                  </dd>
                {/each}
              </div>
            {/if}

            <!-- Phones -->
            {#if allPhones(c).length}
              <div>
                <dt class="text-xs font-medium text-gray-400 dark:text-gray-500 uppercase tracking-wider mb-1.5">Phone</dt>
                {#each allPhones(c) as p}
                  <dd class="flex items-center gap-2 text-sm text-gray-800 dark:text-gray-200 mb-1">
                    {#if p.type}
                      <span class="text-xs text-gray-400 dark:text-gray-500 w-14 flex-shrink-0 capitalize">{p.type}</span>
                    {/if}
                    <a href="tel:{p.number}" class="hover:underline">{p.number}</a>
                  </dd>
                {/each}
              </div>
            {/if}

            <!-- Address -->
            {#if address(c)}
              {@const a = address(c)}
              <div>
                <dt class="text-xs font-medium text-gray-400 dark:text-gray-500 uppercase tracking-wider mb-1.5">Address</dt>
                <dd class="text-sm text-gray-800 dark:text-gray-200 space-y-0.5">
                  {#if a.street}<div>{a.street}</div>{/if}
                  {#if a.city || a.state || a.postal}
                    <div>{[a.city, a.state, a.postal].filter(Boolean).join(', ')}</div>
                  {/if}
                  {#if a.country}<div>{a.country}</div>{/if}
                </dd>
              </div>
            {/if}

            <!-- Birthday -->
            {#if birthday(c)}
              <div>
                <dt class="text-xs font-medium text-gray-400 dark:text-gray-500 uppercase tracking-wider mb-1.5">Birthday</dt>
                <dd class="text-sm text-gray-800 dark:text-gray-200">{birthday(c)}</dd>
              </div>
            {/if}

            <!-- Website -->
            {#if website(c)}
              <div>
                <dt class="text-xs font-medium text-gray-400 dark:text-gray-500 uppercase tracking-wider mb-1.5">Website</dt>
                <dd class="text-sm">
                  <a href={website(c)} target="_blank" rel="noopener noreferrer"
                     class="text-blue-600 dark:text-blue-400 hover:underline break-all">{website(c)}</a>
                </dd>
              </div>
            {/if}

            <!-- Notes -->
            {#if notes(c)}
              <div>
                <dt class="text-xs font-medium text-gray-400 dark:text-gray-500 uppercase tracking-wider mb-1.5">Notes</dt>
                <dd class="text-sm text-gray-800 dark:text-gray-200 whitespace-pre-wrap">{notes(c)}</dd>
              </div>
            {/if}

          </dl>
        </div>
      {:else}
        <div class="flex items-center justify-center h-full text-gray-400 dark:text-gray-500 text-sm">
          Select a contact
        </div>
      {/if}
    </div>

  </div>
</div>

<ContactModal />
<Toasts />
