<script>
  import { onMount, onDestroy } from 'svelte';
  import { page } from '$app/stores';
  import Navbar from '$lib/components/Navbar.svelte';
  import Sidebar from '$lib/components/Sidebar.svelte';
  import MessageList from '$lib/components/MessageList.svelte';
  import MessagePane from '$lib/components/MessagePane.svelte';
  import ComposeModal from '$lib/components/ComposeModal.svelte';
  import ContextMenu from '$lib/components/ContextMenu.svelte';
  import NewFolderModal from '$lib/components/NewFolderModal.svelte';
  import SieveEditor from '$lib/components/SieveEditor.svelte';
  import AppPasswordsModal from '$lib/components/AppPasswordsModal.svelte';
  import MailboxPicker from '$lib/components/MailboxPicker.svelte';
  import Toasts from '$lib/components/Toasts.svelte';
  import SidebarDrawer from '$lib/components/SidebarDrawer.svelte';
  import { isCompact, closeSidebar } from '$lib/stores/viewport.js';
  import { listPaneClass, detailPaneClass } from '$lib/layout.js';
  import {
    mailboxes, selectedMailbox, emails, loading,
    jmapSession, jmapAccountId, selectedEmailId, sidebarWidth, messageListWidth, currentUser,
    composeOpen, composeContext, anyModalOpen, visibleEmails, messageActions,
    shortcutsOpen, selectedEmailIds, mailRefresher
  } from '$lib/stores/mail.js';
  import { getJMAPSession, getMailboxes, getMailboxCounts, getEmails, getAppConfig } from '$lib/api.js';
  import { toast } from '$lib/stores/toast.js';

  const MIN_SIDEBAR  = 180;  const MAX_SIDEBAR  = 380;
  const MIN_MSGLIST  = 220;  const MAX_MSGLIST  = 520;

  const PAGE = 50;

  let dragging       = null;
  let dragStartX     = 0;
  let dragStartWidth = 0;
  let stalwartUrl    = '';
  let initError      = '';

  function startDrag(e, panel) {
    dragging       = panel;
    dragStartX     = e.clientX;
    dragStartWidth = panel === 'sidebar' ? $sidebarWidth : $messageListWidth;
    e.preventDefault();
  }

  function onMouseMove(e) {
    if (!dragging) return;
    const newWidth = dragStartWidth + (e.clientX - dragStartX);
    if (dragging === 'sidebar') {
      sidebarWidth.set(Math.max(MIN_SIDEBAR, Math.min(MAX_SIDEBAR, newWidth)));
    } else {
      messageListWidth.set(Math.max(MIN_MSGLIST, Math.min(MAX_MSGLIST, newWidth)));
    }
  }

  function stopDrag() { dragging = null; }

  onMount(() => {
    window.addEventListener('mousemove', onMouseMove);
    window.addEventListener('mouseup',   stopDrag);
  });
  onDestroy(() => {
    window.removeEventListener('mousemove', onMouseMove);
    window.removeEventListener('mouseup',   stopDrag);
    teardownRealtime();
  });

  // ── Real-time event stream + polling fallback ────────────────────────────
  let es          = null;
  let esTimer     = null;
  let pollTimer   = null;
  let esRetries   = 0;
  let streamLive  = false;

  function teardownRealtime() {
    clearTimeout(esTimer);
    clearInterval(pollTimer);
    esTimer = pollTimer = null;
    try { es?.close(); } catch {}
    es = null;
    streamLive = false;
  }

  function connectStream() {
    if (es?.readyState === 0 || es?.readyState === 1) return; // CONNECTING or OPEN
    clearTimeout(esTimer);
    esTimer = null;
    es = new EventSource('/api/jmap/events');

    const onData = ({ data }) => {
      try {
        const msg = JSON.parse(data);
        // Trigger on any state change for our account, not just Email
        if (msg?.['@type'] === 'StateChange' && msg.changed?.[accountId]) {
          silentRefresh();
        }
      } catch {}
    };

    es.onopen = () => { streamLive = true; esRetries = 0; };
    es.addEventListener('state', onData);
    es.onmessage = onData;
    es.onerror = () => {
      streamLive = false;
      try { es?.close(); } catch {}
      es = null;
      // Exponential backoff with jitter. A fixed 10s retry means every tab in
      // every browser reconnects in lockstep after an outage, which is exactly
      // the thundering herd the server least needs while recovering.
      const base = Math.min(60_000, 2_000 * 2 ** Math.min(esRetries, 5));
      esRetries += 1;
      clearTimeout(esTimer);
      esTimer = setTimeout(connectStream, base + Math.random() * 1_000);
    };
  }

  // Polling exists only to cover the case where SSE cannot connect at all
  // (a proxy that buffers event streams, say). Running it *alongside* a healthy
  // stream just doubles the request load for no new information.
  function startPolling() {
    clearInterval(pollTimer);
    pollTimer = setInterval(() => {
      if (streamLive) return;
      if (typeof document !== 'undefined' && document.hidden) return;
      silentRefresh();
    }, 30_000);
  }

  function onVisibilityChange() {
    if (document.hidden) return;
    // Coming back to the tab: catch up immediately rather than waiting out the
    // rest of the poll interval, and re-establish the stream if it dropped.
    if (!streamLive) connectStream();
    silentRefresh();
  }

  let refreshing = false;

  async function silentRefresh() {
    const mid = $selectedMailbox?.id;
    if (!mid || !accountId || refreshing) return;
    refreshing = true;
    try {
      // Refetch exactly as many messages as the user has scrolled into view.
      // Refetching a flat 50 used to silently discard everything past the first
      // page, so an infinite-scrolled list jumped back to the top on every tick.
      const want = Math.min(Math.max($emails.length, PAGE), 500);
      const [fresh, counts] = await Promise.all([
        getEmails(accountId, mid, 0, want),
        getMailboxCounts(accountId),
      ]);
      if ($selectedMailbox?.id !== mid) return;      // user moved on mid-flight

      const prevIds = new Set($emails.map(e => e.id));
      emails.set(fresh);

      // Preserve sidebar sort order; only update server-owned counters.
      const byId = new Map(counts.map(c => [c.id, c]));
      mailboxes.update(existing => existing.map(mb => {
        const update = byId.get(mb.id);
        return update ? { ...mb, unreadEmails: update.unreadEmails ?? 0 } : mb;
      }));

      // Only notify about emails with IDs not previously in the store AND recently received.
      // Comparing IDs (not lengths) avoids false positives when moves/deletes cause other
      // emails to slide into the fetch window.
      const fiveMinAgo = Date.now() - 5 * 60 * 1000;
      const reallyNew = fresh.filter(
        e => !prevIds.has(e.id) && new Date(e.receivedAt).getTime() > fiveMinAgo
      );
      if (reallyNew.length > 0) notify(reallyNew.length, reallyNew[0].subject ?? '');
    } catch {
      // Background refresh — a failure here is not worth interrupting the user.
    } finally {
      refreshing = false;
    }
  }


  async function notify(count, subject) {
    if (typeof Notification === 'undefined' || Notification.permission !== 'granted') return;
    const title = count === 1 ? 'New message' : `${count} new messages`;
    const opts  = { body: subject, icon: '/icons/icon-192.png', tag: 'mecloud', renotify: true };
    try {
      // Prefer SW notification — required in Chrome when a service worker is active
      if ('serviceWorker' in navigator) {
        const reg = await navigator.serviceWorker.ready;
        await reg.showNotification(title, opts);
      } else {
        new Notification(title, opts);
      }
    } catch {
      try { new Notification(title, opts); } catch {}
    }
  }

  // ── Mail loading ────────────────────────────────────────────────────────────
  let accountId = null;

  // A message the dashboard asked for by id, held until its folder has loaded.
  // loadEmails clears the selection on every folder change, so selecting it any
  // earlier than that would simply be undone.
  let pendingEmailId = null;

  // Track the id, not the object: renaming a folder produces a new object and
  // used to trigger a pointless full reload of its message list.
  let loadedMailboxId = null;
  $: if ($selectedMailbox && accountId && $selectedMailbox.id !== loadedMailboxId) {
    loadedMailboxId = $selectedMailbox.id;
    loadEmails($selectedMailbox.id);
    // Picking a folder from the drawer should reveal its messages rather than
    // leaving the overlay covering them.
    closeSidebar();
  }

  // Sequence guard: switching folders quickly used to let a slow response for
  // the previous folder land last and paint the wrong messages.
  let loadSeq = 0;

  async function loadEmails(mailboxId) {
    const seq = ++loadSeq;
    loading.set(true);
    selectedEmailId.set(null);
    try {
      const list = await getEmails(accountId, mailboxId, 0, PAGE);
      if (seq !== loadSeq) return;
      emails.set(list);
      if (pendingEmailId) {
        // The reading pane fetches by id, so this opens even in the unlikely
        // case that the message sits past the first page of the list.
        selectedEmailId.set(pendingEmailId);
        pendingEmailId = null;
      }
    } catch (e) {
      if (seq === loadSeq) {
        emails.set([]);
        toast(e?.message ?? 'Could not load messages', 'error');
      }
    } finally {
      if (seq === loadSeq) loading.set(false);
    }
  }

  // ── Keyboard shortcuts ──────────────────────────────────────────────────────
  function isTyping(target) {
    if (!target) return false;
    const tag = target.tagName;
    return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || target.isContentEditable;
  }

  function step(delta) {
    const list = $visibleEmails;
    if (!list.length) return;
    const current = list.findIndex(e => e.id === $selectedEmailId);
    const next = current === -1
      ? (delta > 0 ? 0 : list.length - 1)
      : Math.min(list.length - 1, Math.max(0, current + delta));
    selectedEmailId.set(list[next].id);
    document.getElementById(`email-row-${list[next].id}`)
      ?.scrollIntoView({ block: 'nearest' });
  }

  function onKeydown(e) {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (isTyping(e.target)) return;

    // `?` opens help even while a modal is up; everything else stands down.
    if (e.key === '?') { e.preventDefault(); shortcutsOpen.update(v => !v); return; }
    if ($anyModalOpen) return;

    const actions = $messageActions;

    switch (e.key) {
      case 'j': case 'ArrowDown':  e.preventDefault(); step(1);  break;
      case 'k': case 'ArrowUp':    e.preventDefault(); step(-1); break;
      case 'Escape':
        if ($selectedEmailIds.size) selectedEmailIds.set(new Set());
        else selectedEmailId.set(null);
        break;
      case 'c':
        e.preventDefault();
        composeContext.set(null);
        composeOpen.set(true);
        break;
      case '.':
        e.preventDefault();
        silentRefresh();
        break;
      case 'r': if (actions) { e.preventDefault(); actions.reply(); }      break;
      case 'a': if (actions) { e.preventDefault(); actions.replyAll(); }   break;
      case 'f': if (actions) { e.preventDefault(); actions.forward(); }    break;
      case 'u': if (actions) { e.preventDefault(); actions.toggleSeen(); } break;
      case '#':
      case 'Delete':
        if (actions) { e.preventDefault(); actions.remove(); }
        break;
    }
  }

  onMount(async () => {
    mailRefresher.set(silentRefresh);

    // Grab stalwartUrl from config for the navbar link
    const config = await getAppConfig();
    stalwartUrl = config.stalwartUrl ?? '';

    try {
      const session = await getJMAPSession();
      if (!session) return;                 // apiFetch already redirected to login
      jmapSession.set(session);
      accountId = Object.keys(session.accounts ?? {})[0];
      if (!accountId) throw new Error('This account has no mail access.');
      jmapAccountId.set(accountId);

      // Populate the username shown in the navbar
      currentUser.set(session.username || session.accounts[accountId]?.name || '');

      const mboxList = await getMailboxes(accountId);
      const roleOrder = ['inbox', 'sent', 'drafts', 'junk', 'trash', 'archive'];
      mboxList.sort((a, b) => {
        const ai = roleOrder.indexOf(a.role ?? '');
        const bi = roleOrder.indexOf(b.role ?? '');
        if (ai === -1 && bi === -1) return (a.name ?? '').localeCompare(b.name ?? '');
        if (ai === -1) return 1;
        if (bi === -1) return -1;
        return ai - bi;
      });
      mailboxes.set(mboxList);

      // ?email=&mailbox= is how the dashboard hands a message over. Naming the
      // folder as well as the message means the list behind the reading pane is
      // the one the message is actually in.
      const params = $page.url.searchParams;
      pendingEmailId = params.get('email');
      const wanted = params.get('mailbox');

      const start = (wanted && mboxList.find((m) => m.id === wanted))
        ?? mboxList.find((m) => m.role === 'inbox')
        ?? mboxList[0];
      if (start) selectedMailbox.set(start);

      // The stores outlive the page, so that folder may already be loaded from
      // an earlier visit — in which case the reload above never fires and the
      // pending selection has to be applied here rather than waiting for it.
      if (pendingEmailId && loadedMailboxId === start?.id) {
        selectedEmailId.set(pendingEmailId);
        pendingEmailId = null;
      }

      connectStream();
      startPolling();
      document.addEventListener('visibilitychange', onVisibilityChange);
    } catch (err) {
      console.error('Failed to initialise JMAP session:', err);
      initError = err?.message ?? 'Could not reach the mail server.';
    }
  });

  onDestroy(() => {
    mailRefresher.set(null);
    if (typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', onVisibilityChange);
    }
  });
</script>

<svelte:window on:keydown={onKeydown} />

<div
  class="flex flex-col app-shell bg-gray-100 dark:bg-gray-950"
  class:cursor-col-resize={dragging !== null}
>
  <!-- Top navbar -->
  <Navbar {stalwartUrl} />

  {#if initError}
    <div class="flex items-center gap-3 px-4 py-2 flex-shrink-0
                bg-red-50 dark:bg-red-900/25 border-b border-red-200 dark:border-red-800/60">
      <svg class="w-4 h-4 flex-shrink-0 text-red-500" viewBox="0 0 24 24" fill="none"
           stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
        <circle cx="12" cy="12" r="9" /><path d="M12 8v5M12 16.5h.01" />
      </svg>
      <span class="text-xs text-red-800 dark:text-red-200 flex-1">{initError}</span>
      <button on:click={() => location.reload()}
        class="text-xs font-medium px-2.5 py-1 rounded-md
               bg-red-100 dark:bg-red-800/60 hover:bg-red-200 dark:hover:bg-red-800
               text-red-900 dark:text-red-100 transition-colors duration-150">
        Reload
      </button>
    </div>
  {/if}

  <!-- Three-pane area -->
  <div class="flex flex-1 min-h-0 overflow-hidden select-none">

    <!-- Sidebar: in-flow at desktop widths, an overlay drawer below lg -->
    <SidebarDrawer width={$sidebarWidth} cls="overflow-hidden">
      <Sidebar />
    </SidebarDrawer>

    <!-- Drag handle: sidebar ↔ message list -->
    <!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
    <div
      class="flex-shrink-0 h-full cursor-col-resize relative z-10 group hidden lg:block"
      style="width: 5px"
      on:mousedown={(e) => startDrag(e, 'sidebar')}
      role="separator" aria-orientation="vertical" tabindex="0"
      aria-valuenow={$sidebarWidth} aria-valuemin={180} aria-valuemax={380}
    >
      <div class="absolute inset-y-0 -left-1 -right-1
                  group-hover:bg-blue-400/30 dark:group-hover:bg-blue-500/30
                  transition-colors duration-150"></div>
      <div class="absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-gray-200 dark:bg-gray-700"></div>
    </div>

    <!-- Message list. Full width when compact, and hidden entirely once a
         message is open so the reading pane gets the whole screen. -->
    <div
      class="h-full overflow-hidden {listPaneClass($isCompact, !!$selectedEmailId)}"
      style={$isCompact ? '' : `width: ${$messageListWidth}px`}
    >
      <MessageList />
    </div>

    <!-- Drag handle: message list ↔ reading pane -->
    <!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
    <div
      class="flex-shrink-0 h-full cursor-col-resize relative z-10 group hidden lg:block"
      style="width: 5px"
      on:mousedown={(e) => startDrag(e, 'msglist')}
      role="separator" aria-orientation="vertical" tabindex="0"
      aria-valuenow={$messageListWidth} aria-valuemin={220} aria-valuemax={520}
    >
      <div class="absolute inset-y-0 -left-1 -right-1
                  group-hover:bg-blue-400/30 dark:group-hover:bg-blue-500/30
                  transition-colors duration-150"></div>
      <div class="absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-gray-200 dark:bg-gray-700"></div>
    </div>

    <!-- Reading pane. Compact: only shown once something is selected. -->
    <div class="h-full {detailPaneClass($isCompact, !!$selectedEmailId)}">
      <MessagePane />
    </div>

  </div>
</div>

<ComposeModal />
<ContextMenu />
<MailboxPicker />
<NewFolderModal />
<SieveEditor />
<AppPasswordsModal />
<Toasts />
