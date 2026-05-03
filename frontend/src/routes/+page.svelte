<script>
  import { onMount, onDestroy } from 'svelte';
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
  import {
    mailboxes, selectedMailbox, emails, loading,
    jmapSession, jmapAccountId, selectedEmailId, sidebarWidth, messageListWidth, currentUser
  } from '$lib/stores/mail.js';
  import { getJMAPSession, getMailboxes, getEmails, getAppConfig } from '$lib/api.js';

  const MIN_SIDEBAR  = 180;  const MAX_SIDEBAR  = 380;
  const MIN_MSGLIST  = 220;  const MAX_MSGLIST  = 520;

  let dragging       = null;
  let dragStartX     = 0;
  let dragStartWidth = 0;
  let stalwartUrl    = '';

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
  });

  // ── Mail loading ────────────────────────────────────────────────────────────
  let accountId = null;

  $: if ($selectedMailbox && accountId) loadEmails($selectedMailbox.id);

  async function loadEmails(mailboxId) {
    loading.set(true);
    selectedEmailId.set(null);
    try {
      emails.set(await getEmails(accountId, mailboxId));
    } finally {
      loading.set(false);
    }
  }

  onMount(async () => {
    // Grab stalwartUrl from config for the navbar link
    const config = await getAppConfig();
    stalwartUrl = config.stalwartUrl ?? '';

    try {
      const session = await getJMAPSession();
      jmapSession.set(session);
      accountId = Object.keys(session.accounts)[0];
      jmapAccountId.set(accountId);

      // Populate the username shown in the navbar
      currentUser.set(session.username || session.accounts[accountId]?.name || '');

      const mboxList = await getMailboxes(accountId);
      const roleOrder = ['inbox', 'sent', 'drafts', 'junk', 'trash', 'archive'];
      mboxList.sort((a, b) => {
        const ai = roleOrder.indexOf(a.role ?? '');
        const bi = roleOrder.indexOf(b.role ?? '');
        if (ai === -1 && bi === -1) return a.name.localeCompare(b.name);
        if (ai === -1) return 1;
        if (bi === -1) return -1;
        return ai - bi;
      });
      mailboxes.set(mboxList);

      const inbox = mboxList.find((m) => m.role === 'inbox') ?? mboxList[0];
      if (inbox) selectedMailbox.set(inbox);
    } catch (err) {
      console.error('Failed to initialise JMAP session:', err);
    }
  });
</script>

<div
  class="flex flex-col h-screen bg-gray-100 dark:bg-gray-950"
  class:cursor-col-resize={dragging !== null}
>
  <!-- Top navbar -->
  <Navbar {stalwartUrl} />

  <!-- Three-pane area -->
  <div class="flex flex-1 min-h-0 overflow-hidden select-none">

    <!-- Sidebar -->
    <div class="flex-shrink-0 h-full overflow-hidden" style="width: {$sidebarWidth}px">
      <Sidebar />
    </div>

    <!-- Drag handle: sidebar ↔ message list -->
    <!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
    <div
      class="flex-shrink-0 h-full cursor-col-resize relative z-10 group"
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

    <!-- Message list -->
    <div class="flex-shrink-0 h-full overflow-hidden" style="width: {$messageListWidth}px">
      <MessageList />
    </div>

    <!-- Drag handle: message list ↔ reading pane -->
    <!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
    <div
      class="flex-shrink-0 h-full cursor-col-resize relative z-10 group"
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

    <!-- Reading pane -->
    <div class="flex-1 min-w-0 h-full">
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
