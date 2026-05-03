<script>
  import { fly, fade } from 'svelte/transition';
  import {
    selectedEmailId, jmapSession, movePickerOpen,
    mailboxes, selectedMailbox, jmapAccountId, emails,
    composeContext, composeOpen
  } from '$lib/stores/mail.js';
  import { getEmailBody, moveEmail, destroyEmail, markEmailSeen } from '$lib/api.js';
  import { toast } from '$lib/stores/toast.js';
  import Avatar from './Avatar.svelte';
  import DOMPurify from 'dompurify';

  let email = null;
  let bodyHtml = '';
  let loadingEmail = false;

  async function loadEmail(id) {
    if (!id || !$jmapSession) return;
    loadingEmail = true;
    email = null;
    try {
      const accountId = Object.keys($jmapSession.accounts)[0];
      const data = await getEmailBody(accountId, id);
      const sanitize = (rawHtml) => DOMPurify.sanitize(rawHtml, {
        USE_PROFILES: { html: true },
        FORBID_TAGS: ['script', 'iframe', 'object', 'embed', 'form', 'input', 'button'],
        FORBID_ATTR: ['onerror', 'onload', 'onclick', 'onmouseover', 'action'],
        ALLOW_DATA_ATTR: false,
        FORCE_BODY: true,
      });

      if (data?.htmlBody?.length) {
        const partId = data.htmlBody[0].partId;
        const rawHtml = data.bodyValues?.[partId]?.value ?? '';
        bodyHtml = sanitize(rawHtml);
      } else if (data?.textBody?.length) {
        const partId = data.textBody[0].partId;
        const text = data.bodyValues?.[partId]?.value ?? '';
        const rawHtml = `<pre class="whitespace-pre-wrap font-sans text-sm">${text}</pre>`;
        bodyHtml = sanitize(rawHtml);
      } else {
        bodyHtml = '';
      }
      email = data;
      // Auto-mark as read on open
      if (!data?.keywords?.['$seen']) {
        email = { ...data, keywords: { ...(data.keywords ?? {}), '$seen': true } };
        markEmailSeen(accountId, id, true).catch(() => {});
        emails.update(list => list.map(e =>
          e.id === id ? { ...e, keywords: { ...(e.keywords ?? {}), '$seen': true } } : e
        ));
        mailboxes.update(list => list.map(mb =>
          mb.id === $selectedMailbox?.id
            ? { ...mb, unreadEmails: Math.max(0, (mb.unreadEmails ?? 0) - 1) }
            : mb
        ));
      }
    } finally {
      loadingEmail = false;
    }
  }

  $: loadEmail($selectedEmailId);

  function formatAddress(addrs) {
    if (!addrs?.length) return '';
    return addrs.map((a) => (a.name ? `${a.name} <${a.email}>` : a.email)).join(', ');
  }

  function formatDate(dateStr) {
    if (!dateStr) return '';
    return new Date(dateStr).toLocaleString([], {
      weekday: 'short', month: 'short', day: 'numeric',
      hour: '2-digit', minute: '2-digit'
    });
  }

  $: primaryFrom  = email?.from?.[0] ?? null;

  const btnCls = 'px-3 py-1.5 text-xs font-medium rounded-md bg-gray-100 dark:bg-gray-700 hover:bg-gray-200 dark:hover:bg-gray-600 text-gray-700 dark:text-gray-300 transition-colors duration-150';
  $: trashMailbox = $mailboxes.find(m => m.role === 'trash') ?? null;
  $: inTrash      = $selectedMailbox?.role === 'trash';

  // Reply All is only meaningful when there are multiple participants
  $: allAddrs = [
    ...(email?.from ?? []),
    ...(email?.to   ?? []),
    ...(email?.cc   ?? []),
  ].map(a => a.email).filter(Boolean);
  $: canReplyAll = allAddrs.length > 1;

  function quotedHtml(mode) {
    const from = primaryFrom?.name
      ? `${primaryFrom.name} &lt;${primaryFrom.email}&gt;`
      : (primaryFrom?.email ?? '');
    const date = formatDate(email?.receivedAt);

    if (mode === 'forward') {
      return `<br><br>
<div style="border-left:3px solid #ccc;padding:0 0 0 1em;color:#555;margin-top:1em">
  <p style="margin:0 0 0.5em;font-size:0.85em;color:#777">
    -------- Forwarded Message --------<br>
    <b>From:</b> ${from}<br>
    <b>Date:</b> ${date}<br>
    <b>Subject:</b> ${email?.subject ?? ''}<br>
    <b>To:</b> ${formatAddress(email?.to)}
  </p>
  ${bodyHtml}
</div>`;
    }
    // reply / reply_all
    return `<br><br>
<div style="border-left:3px solid #ccc;padding:0 0 0 1em;color:#555;margin-top:1em">
  <p style="margin:0 0 0.5em;font-size:0.85em;color:#777">On ${date}, ${from} wrote:</p>
  ${bodyHtml}
</div>`;
  }

  function openCompose(mode) {
    if (mode === 'reply') {
      composeContext.set({
        mode: 'reply',
        to: email?.from?.[0]?.email ?? '',
        subject: email?.subject ? `Re: ${email.subject}` : '',
        body: quotedHtml('reply'),
      });
    } else if (mode === 'reply_all') {
      composeContext.set({
        mode: 'reply_all',
        to: allAddrs.join(', '),
        subject: email?.subject ? `Re: ${email.subject}` : '',
        body: quotedHtml('reply'),
      });
    } else {
      composeContext.set({
        mode: 'forward',
        to: '',
        subject: email?.subject ? `Fwd: ${email.subject}` : '',
        body: quotedHtml('forward'),
      });
    }
    composeOpen.set(true);
  }

  async function deleteEmail() {
    if (!email) return;
    const emailId = email.id;
    try {
      if (inTrash) {
        await destroyEmail($jmapAccountId, emailId);
        toast('Message deleted permanently', 'success');
      } else if (trashMailbox) {
        await moveEmail($jmapAccountId, emailId, trashMailbox.id, $selectedMailbox?.id);
        toast('Moved to Trash', 'success');
      }
      emails.update(list => list.filter(e => e.id !== emailId));
      selectedEmailId.set(null);
    } catch (e) {
      toast(e?.message ?? 'Delete failed', 'error');
    }
  }

  $: isSeen = !!(email?.keywords?.['$seen']);

  async function toggleSeen() {
    if (!email) return;
    const next = !isSeen;
    try {
      await markEmailSeen($jmapAccountId, email.id, next);
      email = { ...email, keywords: { ...(email.keywords ?? {}), '$seen': next ? true : undefined } };
      emails.update(list => list.map(e =>
        e.id === email.id
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
</script>

<section class="flex flex-col flex-1 min-w-0 h-full bg-white dark:bg-gray-900 overflow-hidden">
  {#if !$selectedEmailId}
    <div class="flex flex-col items-center justify-center h-full gap-3 text-gray-400 dark:text-gray-600 select-none"
         in:fade={{ duration: 200 }}>
      <svg class="w-12 h-12 opacity-40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
        <path d="M21.75 6.75v10.5a2.25 2.25 0 01-2.25 2.25h-15a2.25 2.25 0 01-2.25-2.25V6.75m19.5 0A2.25 2.25 0 0019.5 4.5h-15a2.25 2.25 0 00-2.25 2.25m19.5 0v.243a2.25 2.25 0 01-1.07 1.916l-7.5 4.615a2.25 2.25 0 01-2.36 0L3.32 8.91a2.25 2.25 0 01-1.07-1.916V6.75"/>
      </svg>
      <span class="text-sm">Select a message to read</span>
    </div>

  {:else if loadingEmail}
    <!-- Skeleton loading state -->
    <div class="flex flex-col h-full" in:fade={{ duration: 150 }}>
      <div class="px-6 py-5 border-b border-gray-200 dark:border-gray-700">
        <!-- Avatar + sender skeleton -->
        <div class="flex items-center gap-3 mb-4">
          <div class="w-11 h-11 rounded-full bg-gray-200 dark:bg-gray-700 animate-pulse flex-shrink-0"></div>
          <div class="flex-1 space-y-2">
            <div class="h-4 bg-gray-200 dark:bg-gray-700 rounded animate-pulse w-1/3"></div>
            <div class="h-3 bg-gray-100 dark:bg-gray-700/60 rounded animate-pulse w-1/4"></div>
          </div>
          <div class="h-3 bg-gray-100 dark:bg-gray-700/60 rounded animate-pulse w-20"></div>
        </div>
        <!-- Subject skeleton -->
        <div class="h-5 bg-gray-200 dark:bg-gray-700 rounded animate-pulse w-2/3 mb-2"></div>
        <div class="h-3 bg-gray-100 dark:bg-gray-700/60 rounded animate-pulse w-1/3"></div>
      </div>
      <!-- Body skeleton -->
      <div class="flex-1 px-6 py-5 space-y-3">
        {#each [1, 0.9, 0.95, 0.6, 0.85, 0.7, 1, 0.5] as w}
          <div class="h-3 bg-gray-100 dark:bg-gray-700/50 rounded animate-pulse" style="width: {w * 100}%"></div>
        {/each}
      </div>
    </div>

  {:else if email}
    {#key email.id}
      <div class="flex flex-col h-full" in:fly={{ x: 16, duration: 220, opacity: 0 }}>

        <!-- Header -->
        <div class="px-6 py-4 border-b border-gray-200 dark:border-gray-700 flex-shrink-0">
          <!-- Top row: avatar + sender + date -->
          <div class="flex items-start justify-between gap-3 mb-3">
            <div class="flex items-center gap-3 min-w-0">
              {#if primaryFrom}
                <Avatar name={primaryFrom.name} email={primaryFrom.email} size="lg" />
              {/if}
              <div class="min-w-0">
                <div class="text-sm font-semibold text-gray-900 dark:text-gray-100 truncate text-left">
                  {primaryFrom?.name || primaryFrom?.email || 'Unknown'}
                </div>
                {#if primaryFrom?.name}
                  <div class="text-xs text-gray-500 dark:text-gray-400 truncate">
                    {primaryFrom.email}
                  </div>
                {/if}
              </div>
            </div>
            <span class="text-xs text-gray-400 dark:text-gray-500 flex-shrink-0 mt-0.5">
              {formatDate(email.receivedAt)}
            </span>
          </div>

          <!-- Subject -->
          <h2 class="text-base font-semibold text-gray-900 dark:text-gray-100 mb-2 leading-snug text-left">
            {email.subject || '(no subject)'}
          </h2>

          <!-- To / Cc -->
          <div class="text-xs text-gray-500 dark:text-gray-400 space-y-0.5">
            <div>To: <span class="text-gray-700 dark:text-gray-300">{formatAddress(email.to)}</span></div>
            {#if email.cc?.length}
              <div>Cc: <span class="text-gray-700 dark:text-gray-300">{formatAddress(email.cc)}</span></div>
            {/if}
          </div>

          <!-- Actions -->
          <div class="flex flex-wrap gap-2 mt-3">
            <button on:click={() => openCompose('reply')} class={btnCls}>
              <svg class="w-3.5 h-3.5 inline-block mr-1" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 17 4 12 9 7"/><path d="M20 18v-2a4 4 0 00-4-4H4"/></svg>Reply
            </button>
            <button on:click={() => openCompose('reply_all')} class={btnCls}
              disabled={!canReplyAll} class:opacity-40={!canReplyAll} class:cursor-not-allowed={!canReplyAll}>
              <svg class="w-3.5 h-3.5 inline-block mr-1" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="7 17 2 12 7 7"/><polyline points="12 17 7 12 12 7"/><path d="M20 18v-2a4 4 0 00-4-4H2"/></svg>Reply All
            </button>
            <button on:click={() => openCompose('forward')} class={btnCls}>
              <svg class="w-3.5 h-3.5 inline-block mr-1" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 17 20 12 15 7"/><path d="M4 18v-2a4 4 0 014-4h12"/></svg>Forward
            </button>
            <button on:click={() => movePickerOpen.set(email.id)} class={btnCls}>
              <svg class="w-3.5 h-3.5 inline-block mr-1" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/></svg>Move
            </button>
            <button on:click={toggleSeen} class={btnCls}>
              {#if isSeen}
                <svg class="w-3.5 h-3.5 inline-block mr-1" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7z"/><line x1="3" y1="3" x2="21" y2="21"/></svg>Mark Unread
              {:else}
                <svg class="w-3.5 h-3.5 inline-block mr-1" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7z"/></svg>Mark Read
              {/if}
            </button>
            <button on:click={deleteEmail}
              class="flex items-center px-3 py-1.5 text-xs font-medium rounded-md transition-colors duration-150
                     bg-red-50 dark:bg-red-900/20 hover:bg-red-100 dark:hover:bg-red-900/40
                     text-red-600 dark:text-red-400">
              <svg class="w-3.5 h-3.5 inline-block mr-1" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 01-2 2H8a2 2 0 01-2-2L5 6"/><path d="M10 11v6M14 11v6"/><path d="M9 6V4a1 1 0 011-1h4a1 1 0 011 1v2"/></svg>{inTrash ? 'Delete Permanently' : 'Delete'}
            </button>
          </div>
        </div>

        <!-- Body -->
        <div class="flex-1 overflow-y-auto px-6 py-5">
          <div class="prose dark:prose-invert max-w-none text-sm text-gray-800 dark:text-gray-200 leading-relaxed">
            {@html bodyHtml}
          </div>
        </div>

      </div>
    {/key}
  {/if}
</section>
