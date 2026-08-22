<script>
  import { onMount } from 'svelte';
  import { fade } from 'svelte/transition';
  import { goto } from '$app/navigation';
  import Navbar from '$lib/components/Navbar.svelte';
  import Toasts from '$lib/components/Toasts.svelte';
  import Spinner from '$lib/components/Spinner.svelte';
  import AppIcon from '$lib/components/AppIcon.svelte';
  import ComposeModal from '$lib/components/ComposeModal.svelte';
  import {
    jmapSession, jmapAccountId, currentUser, appName,
    composeOpen, composeContext, selectedMailbox, mailboxes,
  } from '$lib/stores/mail.js';
  import { getJMAPSession, getAppConfig } from '$lib/api.js';
  import { getDashboardSummary, eventEnd } from '$lib/dashboard.js';
  import { formatBytes } from '$lib/fileTypes.js';

  let stalwartUrl = '';
  let summary = null;
  let loading = true;
  let error   = '';

  $: greeting = (() => {
    const h = new Date().getHours();
    if (h < 12) return 'Good morning';
    if (h < 18) return 'Good afternoon';
    return 'Good evening';
  })();

  $: firstName = String($currentUser ?? '').split('@')[0].split(/[.\s_-]/)[0];
  $: displayName = firstName ? firstName.charAt(0).toUpperCase() + firstName.slice(1) : '';

  async function load() {
    if (!$jmapAccountId) return;
    loading = true;
    error = '';
    try {
      summary = await getDashboardSummary($jmapAccountId, $jmapSession);
    } catch (e) {
      error = e?.message ?? 'Could not load your overview.';
    } finally {
      loading = false;
    }
  }

  function openInbox() {
    const inbox = $mailboxes.find((m) => m.role === 'inbox');
    if (inbox) selectedMailbox.set(inbox);
    goto('/mail');
  }

  function compose() {
    composeContext.set(null);
    composeOpen.set(true);
  }

  function eventTime(event) {
    if (event.showWithoutTime) return 'All day';
    const start = new Date(event.start);
    const end = eventEnd(event);
    const fmt = (d) => d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    return end && end.getTime() !== start.getTime() ? `${fmt(start)} – ${fmt(end)}` : fmt(start);
  }

  function eventLocation(event) {
    const first = Object.values(event?.locations ?? {})[0];
    return first?.name ?? '';
  }

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
        error = e?.message ?? 'Could not reach the server.';
        loading = false;
        return;
      }
    }
    await load();
  });

  const card =
    'flex flex-col rounded-2xl bg-white dark:bg-gray-900 ' +
    'ring-1 ring-gray-900/5 dark:ring-white/10 overflow-hidden';
</script>

<div class="flex flex-col app-shell bg-gray-100 dark:bg-gray-950">
  <Navbar {stalwartUrl} />

  <main class="flex-1 min-h-0 overflow-y-auto">
    <div class="max-w-5xl mx-auto px-4 sm:px-6 py-6 sm:py-8 pb-safe">

      <!-- Greeting -->
      <header class="mb-6">
        <h1 class="text-xl sm:text-2xl font-semibold text-gray-900 dark:text-gray-50">
          {greeting}{displayName ? `, ${displayName}` : ''}
        </h1>
        <p class="text-sm text-gray-500 dark:text-gray-400 mt-1">
          {new Date().toLocaleDateString([], {
            weekday: 'long', day: 'numeric', month: 'long',
          })}
        </p>
      </header>

      {#if error}
        <div class="{card} p-6 items-center text-center gap-3">
          <p class="text-sm text-gray-500 dark:text-gray-400">{error}</p>
          <button on:click={load}
            class="text-xs font-medium px-3 py-1.5 rounded-lg bg-gray-100 dark:bg-gray-700
                   hover:bg-gray-200 dark:hover:bg-gray-600 text-gray-700 dark:text-gray-200
                   transition-colors duration-150">Try again</button>
        </div>

      {:else if loading}
        <div class="grid grid-cols-2 lg:grid-cols-4 gap-3 sm:gap-4">
          {#each Array(4) as _}
            <div class="{card} p-4 gap-3">
              <div class="h-4 w-4 rounded bg-gray-200 dark:bg-gray-700 animate-pulse"></div>
              <div class="h-7 w-16 rounded bg-gray-200 dark:bg-gray-700 animate-pulse"></div>
              <div class="h-3 w-20 rounded bg-gray-100 dark:bg-gray-700/60 animate-pulse"></div>
            </div>
          {/each}
        </div>

      {:else if summary}
        <div in:fade={{ duration: 150 }}>

          <!-- ── Stat tiles ─────────────────────────────────────────────── -->
          <div class="grid grid-cols-2 lg:grid-cols-4 gap-3 sm:gap-4">
            <button on:click={openInbox} class="{card} p-4 text-left
                    hover:ring-blue-300 dark:hover:ring-blue-700 transition-shadow duration-150">
              <AppIcon id="mail" cls="w-4 h-4 text-blue-500 dark:text-blue-400" />
              <span class="mt-2 text-2xl font-semibold text-gray-900 dark:text-gray-50 tabular-nums">
                {summary.mail.unread}
              </span>
              <span class="text-xs text-gray-500 dark:text-gray-400 mt-0.5">
                unread in Inbox
              </span>
              <span class="text-[11px] text-gray-400 dark:text-gray-600 mt-1">
                {summary.mail.inboxTotal.toLocaleString()} message{summary.mail.inboxTotal === 1 ? '' : 's'}
              </span>
            </button>

            {#if summary.files}
              <button on:click={() => goto('/files')} class="{card} p-4 text-left
                      hover:ring-blue-300 dark:hover:ring-blue-700 transition-shadow duration-150">
                <AppIcon id="files" cls="w-4 h-4 text-amber-500 dark:text-amber-400" />
                <span class="mt-2 text-2xl font-semibold text-gray-900 dark:text-gray-50 tabular-nums">
                  {formatBytes(summary.files.bytes)}
                </span>
                <span class="text-xs text-gray-500 dark:text-gray-400 mt-0.5">stored in Files</span>
                <span class="text-[11px] text-gray-400 dark:text-gray-600 mt-1">
                  {summary.files.files.toLocaleString()} file{summary.files.files === 1 ? '' : 's'}
                  · {summary.files.folders} folder{summary.files.folders === 1 ? '' : 's'}
                </span>
              </button>
            {/if}

            {#if summary.contacts !== null}
              <button on:click={() => goto('/contacts')} class="{card} p-4 text-left
                      hover:ring-blue-300 dark:hover:ring-blue-700 transition-shadow duration-150">
                <AppIcon id="contacts" cls="w-4 h-4 text-green-500 dark:text-green-400" />
                <span class="mt-2 text-2xl font-semibold text-gray-900 dark:text-gray-50 tabular-nums">
                  {summary.contacts.toLocaleString()}
                </span>
                <span class="text-xs text-gray-500 dark:text-gray-400 mt-0.5">
                  contact{summary.contacts === 1 ? '' : 's'}
                </span>
              </button>
            {/if}

            {#if summary.events !== null}
              <button on:click={() => goto('/calendar')} class="{card} p-4 text-left
                      hover:ring-blue-300 dark:hover:ring-blue-700 transition-shadow duration-150">
                <AppIcon id="calendar" cls="w-4 h-4 text-purple-500 dark:text-purple-400" />
                <span class="mt-2 text-2xl font-semibold text-gray-900 dark:text-gray-50 tabular-nums">
                  {summary.events.length}
                </span>
                <span class="text-xs text-gray-500 dark:text-gray-400 mt-0.5">
                  event{summary.events.length === 1 ? '' : 's'} today
                </span>
              </button>
            {/if}
          </div>

          <!-- ── Today + quick actions ──────────────────────────────────── -->
          <div class="grid lg:grid-cols-3 gap-3 sm:gap-4 mt-3 sm:mt-4">

            {#if summary.events !== null}
              <section class="{card} lg:col-span-2">
                <div class="flex items-center justify-between px-4 py-3
                            border-b border-gray-100 dark:border-gray-800">
                  <h2 class="text-xs font-semibold uppercase tracking-wider
                             text-gray-500 dark:text-gray-400">Today</h2>
                  <button on:click={() => goto('/calendar')}
                    class="text-xs text-blue-600 dark:text-blue-400 hover:underline">
                    Calendar
                  </button>
                </div>

                {#if summary.events.length === 0}
                  <div class="flex flex-col items-center justify-center gap-2 py-10
                              text-gray-400 dark:text-gray-500">
                    <AppIcon id="calendar" cls="w-8 h-8 opacity-40" />
                    <p class="text-sm">Nothing scheduled today</p>
                  </div>
                {:else}
                  <ul class="divide-y divide-gray-100 dark:divide-gray-800">
                    {#each summary.events as event (event.id)}
                      <li class="flex items-start gap-3 px-4 py-3">
                        <span class="w-24 flex-shrink-0 text-xs tabular-nums
                                     text-gray-500 dark:text-gray-400 pt-0.5">
                          {eventTime(event)}
                        </span>
                        <span class="w-1 self-stretch rounded-full bg-purple-400 dark:bg-purple-500
                                     flex-shrink-0"></span>
                        <div class="min-w-0 flex-1">
                          <p class="text-sm text-gray-800 dark:text-gray-100 truncate">
                            {event.title || '(no title)'}
                          </p>
                          {#if eventLocation(event)}
                            <p class="text-xs text-gray-400 dark:text-gray-500 truncate mt-0.5">
                              {eventLocation(event)}
                            </p>
                          {/if}
                        </div>
                      </li>
                    {/each}
                  </ul>
                {/if}
              </section>
            {/if}

            <section class="{card}">
              <div class="px-4 py-3 border-b border-gray-100 dark:border-gray-800">
                <h2 class="text-xs font-semibold uppercase tracking-wider
                           text-gray-500 dark:text-gray-400">Quick actions</h2>
              </div>
              <div class="p-2 grid grid-cols-2 lg:grid-cols-1 gap-1">
                <button on:click={compose}
                  class="flex items-center gap-2.5 px-3 py-2.5 rounded-lg text-sm text-left
                         text-gray-700 dark:text-gray-200
                         hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors duration-150">
                  <AppIcon id="mail" cls="w-4 h-4 text-blue-500 flex-shrink-0" />
                  Compose
                </button>
                <button on:click={() => goto('/notes')}
                  class="flex items-center gap-2.5 px-3 py-2.5 rounded-lg text-sm text-left
                         text-gray-700 dark:text-gray-200
                         hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors duration-150">
                  <AppIcon id="notes" cls="w-4 h-4 text-teal-500 flex-shrink-0" />
                  Notes{summary.files ? ` (${summary.files.notes})` : ''}
                </button>
                <button on:click={() => goto('/files')}
                  class="flex items-center gap-2.5 px-3 py-2.5 rounded-lg text-sm text-left
                         text-gray-700 dark:text-gray-200
                         hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors duration-150">
                  <AppIcon id="files" cls="w-4 h-4 text-amber-500 flex-shrink-0" />
                  Upload a file
                </button>
                <button on:click={() => goto('/calendar')}
                  class="flex items-center gap-2.5 px-3 py-2.5 rounded-lg text-sm text-left
                         text-gray-700 dark:text-gray-200
                         hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors duration-150">
                  <AppIcon id="calendar" cls="w-4 h-4 text-purple-500 flex-shrink-0" />
                  New event
                </button>
              </div>
            </section>
          </div>

          <!-- ── Mail breakdown ─────────────────────────────────────────── -->
          <section class="{card} mt-3 sm:mt-4">
            <div class="flex items-center justify-between px-4 py-3
                        border-b border-gray-100 dark:border-gray-800">
              <h2 class="text-xs font-semibold uppercase tracking-wider
                         text-gray-500 dark:text-gray-400">Mail</h2>
              <button on:click={openInbox}
                class="text-xs text-blue-600 dark:text-blue-400 hover:underline">Open</button>
            </div>
            <dl class="grid grid-cols-2 sm:grid-cols-4 divide-x divide-y sm:divide-y-0
                       divide-gray-100 dark:divide-gray-800">
              {#each [
                ['Unread', summary.mail.unreadAll],
                ['Total messages', summary.mail.total],
                ['Drafts', summary.mail.drafts],
                ['Folders', summary.mail.mailboxes],
              ] as [label, value]}
                <div class="px-4 py-3">
                  <dt class="text-[11px] text-gray-400 dark:text-gray-500">{label}</dt>
                  <dd class="text-lg font-semibold text-gray-900 dark:text-gray-100 tabular-nums">
                    {Number(value).toLocaleString()}
                  </dd>
                </div>
              {/each}
            </dl>
          </section>

          <p class="text-center text-[11px] text-gray-400 dark:text-gray-600 mt-6">
            {$appName}
          </p>
        </div>
      {/if}
    </div>
  </main>
</div>

<ComposeModal />
<Toasts />
