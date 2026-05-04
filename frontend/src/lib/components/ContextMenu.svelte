<script>
  import { fly } from 'svelte/transition';
  import {
    contextMenu, movePickerOpen, emails, mailboxes,
    selectedMailbox, jmapAccountId, selectedEmailId,
    composeContext, composeOpen
  } from '$lib/stores/mail.js';
  import { moveEmail, destroyEmail, markEmailSeen } from '$lib/api.js';
  import { refreshMailboxCounts } from '$lib/mailboxRefresh.js';
  import { toast } from '$lib/stores/toast.js';

  $: email        = $emails.find(e => e.id === $contextMenu?.emailId) ?? null;
  $: trashMailbox = $mailboxes.find(m => m.role === 'trash') ?? null;
  $: inTrash      = $selectedMailbox?.role === 'trash';

  function close() { contextMenu.set(null); }

  function openPicker() {
    if ($contextMenu) movePickerOpen.set($contextMenu.emailId);
    close();
  }

  function reply() {
    if (!email) return;
    const from = email.from?.[0];
    composeContext.set({
      mode: 'reply',
      to: from?.email ?? '',
      subject: email.subject ? `Re: ${email.subject}` : '',
    });
    composeOpen.set(true);
    close();
  }

  function replyAll() {
    if (!email) return;
    const allAddrs = [
      ...(email.from ?? []),
      ...(email.to   ?? []),
      ...(email.cc   ?? []),
    ].map(a => a.email).filter(Boolean);
    composeContext.set({
      mode: 'reply_all',
      to: allAddrs.join(', '),
      subject: email.subject ? `Re: ${email.subject}` : '',
    });
    composeOpen.set(true);
    close();
  }

  function forward() {
    if (!email) return;
    composeContext.set({
      mode: 'forward',
      to: '',
      subject: email.subject ? `Fwd: ${email.subject}` : '',
    });
    composeOpen.set(true);
    close();
  }

  async function del() {
    const emailId = $contextMenu?.emailId;
    if (!emailId) return;
    close();
    try {
      if (inTrash) {
        await destroyEmail($jmapAccountId, emailId);
        toast('Message deleted permanently', 'success');
      } else if (trashMailbox) {
        await moveEmail($jmapAccountId, emailId, trashMailbox.id, $selectedMailbox?.id);
        toast('Moved to Trash', 'success');
      }
      emails.update(list => list.filter(e => e.id !== emailId));
      if ($selectedEmailId === emailId) selectedEmailId.set(null);
      await refreshMailboxCounts();
    } catch (e) {
      toast(e?.message ?? 'Delete failed', 'error');
    }
  }

  $: isSeen = !!(email?.keywords?.['$seen']);

  async function toggleSeen() {
    const emailId = $contextMenu?.emailId;
    if (!emailId) return;
    const next = !isSeen;
    close();
    try {
      await markEmailSeen($jmapAccountId, emailId, next);
      emails.update(list => list.map(e =>
        e.id === emailId
          ? { ...e, keywords: { ...(e.keywords ?? {}), '$seen': next ? true : undefined } }
          : e
      ));
      mailboxes.update(list => list.map(mb =>
        mb.id === $selectedMailbox?.id
          ? { ...mb, unreadEmails: Math.max(0, (mb.unreadEmails ?? 0) + (next ? -1 : 1)) }
          : mb
      ));
    } catch (e) {
      toast(e?.message ?? 'Failed to update read status', 'error');
    }
  }

  // Disable Reply All when there's only one unique address
  $: replyAllAddrs = [
    ...(email?.from ?? []),
    ...(email?.to   ?? []),
    ...(email?.cc   ?? []),
  ].map(a => a.email).filter(Boolean);
  $: canReplyAll = replyAllAddrs.length > 1;

  const itemCls = `flex items-center gap-2.5 w-full px-4 py-2 text-sm text-gray-700 dark:text-gray-300
    hover:bg-gray-50 dark:hover:bg-gray-700/60 transition-colors duration-100 text-left`;
</script>

{#if $contextMenu}
  <div class="fixed inset-0 z-40" on:click={close} aria-hidden="true"></div>

  <div
    class="fixed z-50 min-w-[180px] rounded-lg shadow-lg ring-1
           bg-white dark:bg-gray-800 ring-black/5 dark:ring-white/10 py-1 overflow-hidden"
    style="left: {$contextMenu.x}px; top: {$contextMenu.y}px"
    in:fly={{ y: -4, duration: 120 }}
  >
    <button on:click={reply} class="{itemCls}">
      <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 17 4 12 9 7"/><path d="M20 18v-2a4 4 0 00-4-4H4"/></svg>
      Reply
    </button>
    <button on:click={replyAll} class="{itemCls}" disabled={!canReplyAll}
      class:opacity-40={!canReplyAll} class:cursor-not-allowed={!canReplyAll}>
      <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><polyline points="7 17 2 12 7 7"/><polyline points="12 17 7 12 12 7"/><path d="M20 18v-2a4 4 0 00-4-4H2"/></svg>
      Reply All
    </button>
    <button on:click={forward} class="{itemCls}">
      <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 17 20 12 15 7"/><path d="M4 18v-2a4 4 0 014-4h12"/></svg>
      Forward
    </button>

    <button on:click={toggleSeen} class="{itemCls}">
      {#if isSeen}
        <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7z"/><line x1="3" y1="3" x2="21" y2="21"/></svg>
        Mark Unread
      {:else}
        <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7z"/></svg>
        Mark Read
      {/if}
    </button>

    <div class="my-1 border-t border-gray-100 dark:border-gray-700"></div>

    <button on:click={openPicker} class="{itemCls}">
      <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/></svg>
      Move to folder…
    </button>

    <div class="my-1 border-t border-gray-100 dark:border-gray-700"></div>

    <button on:click={del}
      class="flex items-center gap-2.5 w-full px-4 py-2 text-sm text-left
             text-red-600 dark:text-red-400
             hover:bg-red-50 dark:hover:bg-red-900/20 transition-colors duration-100"
    >
      <svg class="w-4 h-4 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 01-2 2H8a2 2 0 01-2-2L5 6"/><path d="M10 11v6M14 11v6"/><path d="M9 6V4a1 1 0 011-1h4a1 1 0 011 1v2"/></svg>
      {inTrash ? 'Delete Permanently' : 'Delete'}
    </button>
  </div>
{/if}
