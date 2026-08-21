<script>
  import { onDestroy } from 'svelte';
  import { fly, fade } from 'svelte/transition';
  import {
    selectedEmailId, jmapSession, movePickerOpen,
    mailboxes, selectedMailbox, jmapAccountId, emails,
    composeContext, composeOpen, darkMode, messageActions
  } from '$lib/stores/mail.js';
  import { getEmailBody, moveEmail, destroyEmail, markEmailSeen } from '$lib/api.js';
  import { refreshMailboxCounts } from '$lib/mailboxRefresh.js';
  import { toast } from '$lib/stores/toast.js';
  import { sanitizeEmailHtml, escapeText, decodeEntities } from '$lib/sanitize.js';
  import { formatBytes } from '$lib/fileTypes.js';
  import { downloadNode } from '$lib/files.js';
  import { isTrustedSender, trustSender } from '$lib/trustedSenders.js';
  import Avatar from './Avatar.svelte';
  import Spinner from './Spinner.svelte';
  import { isCompact } from '$lib/stores/viewport.js';

  let email = null;
  let frameContent = '';    // sanitised content cached for dark-mode rebuilds
  let framePlain   = false; // true → frameContent is escaped plain text (needs <pre>)
  let loadingEmail = false;
  let loadError    = '';

  // Remote-content state for the message on screen.
  let rawHtml       = '';    // kept so "show images" can re-sanitise without a refetch
  let blockedCount  = 0;
  let imagesAllowed = false;

  // Per-action busy flags so each control can show its own spinner rather than
  // freezing the whole pane.
  let deleting   = false;
  let togglingSeen = false;

  // Theme-aware base styles injected into every frame document.
  function frameBase(dark) {
    return (
      '<meta charset="utf-8">' +
      '<meta name="viewport" content="width=device-width,initial-scale=1">' +
      '<base target="_blank">' +
      '<style>' +
        `body{background:${dark ? '#1f2937' : '#fff'};color:${dark ? '#e5e7eb' : '#111827'};` +
             'font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Helvetica,Arial,sans-serif;' +
             'font-size:14px;line-height:1.5;word-break:break-word;padding:16px 24px;margin:0}' +
        'img{max-width:100%;height:auto}' +
        `a{color:${dark ? '#60a5fa' : '#2563eb'}}` +
        'pre{white-space:pre-wrap;font-family:inherit;font-size:inherit;margin:0;line-height:1.6}' +
        'table{max-width:100%}' +
        `blockquote{border-left:3px solid ${dark ? '#4b5563' : '#d1d5db'};` +
                   `margin:.5em 0;padding:0 0 0 1em;color:${dark ? '#9ca3af' : '#6b7280'}}` +
      '</style>'
    );
  }

  function wrapDoc(html, dark) {
    const base = frameBase(dark);
    if (/<html[\s>]/i.test(html)) {
      return html.replace(/(<head[^>]*>)/i, `$1${base}`);
    }
    return `<!DOCTYPE html><html><head>${base}</head><body>${html}</body></html>`;
  }

  // Rebuild frameDoc whenever content or dark mode changes.
  $: frameDoc = frameContent
    ? (framePlain
        ? `<!DOCTYPE html><html><head>${frameBase($darkMode)}</head><body><pre>${frameContent}</pre></body></html>`
        : wrapDoc(frameContent, $darkMode))
    : '';

  /** Re-sanitise `rawHtml` at the current trust level and refresh the frame. */
  function renderBody() {
    if (!rawHtml) return;
    const framed = sanitizeEmailHtml(rawHtml, {
      allowRemote: imagesAllowed,
      wholeDocument: true,
    });
    frameContent = framed.html;
    blockedCount = framed.blocked;
    framePlain   = false;
  }

  /**
   * The body as a fragment, for quoting into a reply or forward. Computed on
   * demand rather than alongside every render: sanitising a large message twice
   * on open was pure waste when most messages are never replied to.
   */
  function quotedBodyHtml() {
    if (framePlain) return `<pre>${frameContent}</pre>`;
    if (!rawHtml) return '';
    return sanitizeEmailHtml(rawHtml, { allowRemote: imagesAllowed }).html;
  }

  function showImages() {
    imagesAllowed = true;
    renderBody();
  }

  function alwaysShowImages() {
    if (primaryFrom?.email) trustSender(primaryFrom.email);
    showImages();
  }

  // Guards against a slow request for message A landing after the user has
  // already clicked message B and overwriting the pane with stale content.
  let loadSeq = 0;

  async function loadEmail(id) {
    if (!id || !$jmapSession) return;
    const seq = ++loadSeq;

    loadingEmail = true;
    loadError    = '';
    email = null;
    frameContent  = '';
    rawHtml       = '';
    framePlain    = false;
    blockedCount  = 0;
    imagesAllowed = false;

    try {
      const accountId = Object.keys($jmapSession.accounts)[0];
      const data = await getEmailBody(accountId, id);
      if (seq !== loadSeq) return;          // superseded by a newer selection
      if (!data) {
        loadError = 'This message could not be loaded.';
        return;
      }

      // Trust decisions are per sender, so this has to happen before the first
      // render rather than after the banner has already flashed up.
      imagesAllowed = isTrustedSender(data.from?.[0]?.email);

      // Fetch the htmlBody value, but only treat it as real HTML if it
      // actually contains markup — Stalwart sometimes points htmlBody at a
      // text/plain part, and parsing plain text as HTML collapses newlines.
      let html = '';
      if (data.htmlBody?.length) {
        const pid = data.htmlBody[0].partId;
        const val = data.bodyValues?.[pid]?.value ?? '';
        if (val.includes('</')) html = val;
      }

      if (html) {
        rawHtml = html;
        renderBody();
      } else {
        // Plain-text path — prefer textBody, fall back to htmlBody value if needed
        let text = '';
        if (data.textBody?.length) {
          text = data.bodyValues?.[data.textBody[0].partId]?.value ?? '';
        }
        if (!text && data.htmlBody?.length) {
          text = data.bodyValues?.[data.htmlBody[0].partId]?.value ?? '';
        }
        const escaped = escapeText(decodeEntities(text));
        frameContent = escaped;
        framePlain   = true;
      }

      email = data;
      // Auto-mark as read on open
      if (!data.keywords?.['$seen']) {
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
    } catch (e) {
      if (seq === loadSeq) loadError = e?.message ?? 'This message could not be loaded.';
    } finally {
      if (seq === loadSeq) loadingEmail = false;
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
    // Every field below comes from the message headers, which the sender
    // controls. The quote block ends up as innerHTML in the compose editor, and
    // innerHTML assignment fires inline handlers such as <img onerror> even
    // though it will not run a <script> tag — so these must be escaped, not
    // interpolated raw. Only `body` is pre-sanitised markup.
    const from = primaryFrom?.name
      ? `${escapeText(primaryFrom.name)} &lt;${escapeText(primaryFrom.email ?? '')}&gt;`
      : escapeText(primaryFrom?.email ?? '');
    const date    = escapeText(formatDate(email?.receivedAt));
    const subject = escapeText(email?.subject ?? '');
    const to      = escapeText(formatAddress(email?.to));
    const body    = quotedBodyHtml();

    if (mode === 'forward') {
      return `<br><br>
<div style="border-left:3px solid #ccc;padding:0 0 0 1em;color:#555;margin-top:1em">
  <p style="margin:0 0 0.5em;font-size:0.85em;color:#777">
    -------- Forwarded Message --------<br>
    <b>From:</b> ${from}<br>
    <b>Date:</b> ${date}<br>
    <b>Subject:</b> ${subject}<br>
    <b>To:</b> ${to}
  </p>
  ${body}
</div>`;
    }
    // reply / reply_all
    return `<br><br>
<div style="border-left:3px solid #ccc;padding:0 0 0 1em;color:#555;margin-top:1em">
  <p style="margin:0 0 0.5em;font-size:0.85em;color:#777">On ${date}, ${from} wrote:</p>
  ${body}
</div>`;
  }

  // Reply-To wins over From when the sender asked for it (mailing lists rely
  // on this), which is what every other client does.
  $: replyTarget = email?.replyTo?.[0]?.email ?? email?.from?.[0]?.email ?? '';

  function openCompose(mode) {
    // Carrying messageId/references through is what keeps the reply in the
    // same conversation in the recipient's client.
    const thread = {
      inReplyTo: email?.messageId?.[0] ?? null,
      references: email?.references ?? [],
    };

    if (mode === 'reply') {
      composeContext.set({
        mode: 'reply',
        to: replyTarget,
        subject: email?.subject ? `Re: ${email.subject}` : '',
        body: quotedHtml('reply'),
        ...thread,
      });
    } else if (mode === 'reply_all') {
      composeContext.set({
        mode: 'reply_all',
        to: allAddrs.join(', '),
        subject: email?.subject ? `Re: ${email.subject}` : '',
        body: quotedHtml('reply'),
        ...thread,
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
    if (!email || deleting) return;
    const emailId = email.id;
    deleting = true;
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
      await refreshMailboxCounts();
    } catch (e) {
      toast(e?.message ?? 'Delete failed', 'error');
    } finally {
      deleting = false;
    }
  }

  $: isSeen = !!(email?.keywords?.['$seen']);

  async function toggleSeen() {
    if (!email || togglingSeen) return;
    const next = !isSeen;
    togglingSeen = true;
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
    } finally {
      togglingSeen = false;
    }
  }

  $: attachments = (email?.attachments ?? []).filter(a => a?.disposition !== 'inline');

  // Publish this message's actions so the page-level keyboard handler can drive
  // them (r / a / f / u / #) without either component knowing about the other.
  $: messageActions.set(email ? {
    reply:     () => openCompose('reply'),
    replyAll:  () => { if (canReplyAll) openCompose('reply_all'); },
    forward:   () => openCompose('forward'),
    toggleSeen,
    remove:    deleteEmail,
  } : null);

  onDestroy(() => messageActions.set(null));
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

      <div class="flex items-center justify-center gap-2 pb-6 text-xs text-gray-400 dark:text-gray-500">
        <Spinner size="xs" label="" />
        Loading message…
      </div>
    </div>

  {:else if loadError}
    <div class="flex flex-col items-center justify-center h-full gap-3 px-6 text-center"
         in:fade={{ duration: 150 }}>
      <svg class="w-10 h-10 text-gray-300 dark:text-gray-600" viewBox="0 0 24 24" fill="none"
           stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
        <circle cx="12" cy="12" r="9" /><path d="M12 8v5M12 16.5h.01" />
      </svg>
      <p class="text-sm text-gray-500 dark:text-gray-400">{loadError}</p>
      <button
        on:click={() => loadEmail($selectedEmailId)}
        class="text-xs font-medium px-3 py-1.5 rounded-lg
               bg-gray-100 dark:bg-gray-700 hover:bg-gray-200 dark:hover:bg-gray-600
               text-gray-700 dark:text-gray-200 transition-colors duration-150"
      >Try again</button>
    </div>

  {:else if email}
    {#key email.id}
      <div class="flex flex-col h-full" in:fly={{ x: 16, duration: 220, opacity: 0 }}>

        <!-- Header -->
        <div class="px-6 py-4 border-b border-gray-200 dark:border-gray-700 flex-shrink-0">
          {#if $isCompact}
            <button
              on:click={() => selectedEmailId.set(null)}
              class="flex items-center gap-1 -ml-1 mb-2 px-1.5 py-1 rounded-lg text-xs font-medium
                     text-blue-600 dark:text-blue-400
                     hover:bg-blue-50 dark:hover:bg-blue-900/30 transition-colors duration-150"
            >
              <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                   stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="m15 18-6-6 6-6" />
              </svg>
              {$selectedMailbox?.name ?? 'Back'}
            </button>
          {/if}

          <!-- Top row: avatar + sender + date -->
          <div class="flex items-start justify-between gap-3 mb-3">
            <div class="flex items-center gap-3 min-w-0">
              {#if primaryFrom}
                <Avatar name={primaryFrom.name} email={primaryFrom.email} size="lg" />
              {/if}
              <div class="min-w-0">
                <div class="text-sm font-semibold text-gray-900 dark:text-gray-100 truncate">
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
          <h2 class="text-base font-semibold text-gray-900 dark:text-gray-100 mb-2 leading-snug">
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
            <button on:click={toggleSeen} class={btnCls} disabled={togglingSeen}
              class:opacity-60={togglingSeen}>
              {#if togglingSeen}
                <Spinner size="xs" label="" cls="mr-1 align-[-1px]" />
              {:else if isSeen}
                <svg class="w-3.5 h-3.5 inline-block mr-1" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7z"/><line x1="3" y1="3" x2="21" y2="21"/></svg>
              {:else}
                <svg class="w-3.5 h-3.5 inline-block mr-1" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7z"/></svg>
              {/if}
              {isSeen ? 'Mark Unread' : 'Mark Read'}
            </button>
            <button on:click={deleteEmail} disabled={deleting}
              class="flex items-center px-3 py-1.5 text-xs font-medium rounded-md transition-colors duration-150
                     bg-red-50 dark:bg-red-900/20 hover:bg-red-100 dark:hover:bg-red-900/40
                     text-red-600 dark:text-red-400
                     disabled:opacity-60 disabled:cursor-not-allowed">
              {#if deleting}
                <Spinner size="xs" label="" accent="border-t-red-500" cls="mr-1" />
              {:else}
                <svg class="w-3.5 h-3.5 inline-block mr-1" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 01-2 2H8a2 2 0 01-2-2L5 6"/><path d="M10 11v6M14 11v6"/><path d="M9 6V4a1 1 0 011-1h4a1 1 0 011 1v2"/></svg>
              {/if}
              {deleting ? 'Deleting…' : inTrash ? 'Delete Permanently' : 'Delete'}
            </button>
          </div>

          <!-- Attachments -->
          {#if attachments.length}
            <div class="flex flex-wrap items-center gap-2 mt-3 pt-3 border-t border-gray-100 dark:border-gray-700/60">
              <span class="text-xs text-gray-400 dark:text-gray-500 flex items-center gap-1">
                <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M21.44 11.05l-9.19 9.19a6 6 0 01-8.49-8.49l9.19-9.19a4 4 0 015.66 5.66l-9.2 9.19a2 2 0 01-2.83-2.83l8.49-8.48"/>
                </svg>
                {attachments.length}
              </span>
              {#each attachments as att}
                <button
                  on:click={() => downloadNode({ blobId: att.blobId, name: att.name, type: att.type })}
                  disabled={!att.blobId}
                  title={att.blobId ? `Download ${att.name || 'attachment'}` : 'This attachment has no downloadable content'}
                  class="inline-flex items-center gap-1.5 max-w-[14rem] px-2 py-1 rounded-md
                         bg-gray-100 dark:bg-gray-700/60 text-xs text-gray-600 dark:text-gray-300
                         hover:bg-gray-200 dark:hover:bg-gray-600
                         disabled:opacity-50 disabled:cursor-not-allowed
                         transition-colors duration-150"
                >
                  <svg class="w-3 h-3 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                       stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M12 3v12m0 0-4-4m4 4 4-4M5 19h14" />
                  </svg>
                  <span class="truncate">{att.name || 'attachment'}</span>
                  {#if att.size}
                    <span class="text-gray-400 dark:text-gray-500 flex-shrink-0">{formatBytes(att.size)}</span>
                  {/if}
                </button>
              {/each}
            </div>
          {/if}
        </div>

        <!-- Remote-content notice: a remote <img> is a read receipt the sender
             gets without asking, so nothing off-origin loads until requested. -->
        {#if blockedCount > 0 && !imagesAllowed}
          <div class="flex flex-wrap items-center gap-x-3 gap-y-2 px-6 py-2.5 flex-shrink-0
                      bg-amber-50 dark:bg-amber-900/20
                      border-b border-amber-200 dark:border-amber-800/50"
               in:fade={{ duration: 150 }}>
            <svg class="w-4 h-4 flex-shrink-0 text-amber-500 dark:text-amber-400" viewBox="0 0 24 24"
                 fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
              <path d="M17.94 17.94A10.07 10.07 0 0112 20c-7 0-11-8-11-8a18.45 18.45 0 015.06-5.94M9.9 4.24A9.12 9.12 0 0112 4c7 0 11 8 11 8a18.5 18.5 0 01-2.16 3.19"/>
              <path d="M14.12 14.12a3 3 0 11-4.24-4.24"/><line x1="1" y1="1" x2="23" y2="23"/>
            </svg>
            <span class="text-xs text-amber-800 dark:text-amber-200 flex-1 min-w-0">
              {blockedCount} remote {blockedCount === 1 ? 'image was' : 'images were'} blocked to stop
              the sender tracking when you opened this message.
            </span>
            <div class="flex items-center gap-2 flex-shrink-0">
              <button on:click={showImages}
                class="text-xs font-medium px-2.5 py-1 rounded-md
                       bg-amber-100 dark:bg-amber-800/60 hover:bg-amber-200 dark:hover:bg-amber-800
                       text-amber-900 dark:text-amber-100 transition-colors duration-150">
                Show images
              </button>
              {#if primaryFrom?.email}
                <button on:click={alwaysShowImages}
                  class="text-xs text-amber-700 dark:text-amber-300 hover:underline">
                  Always from this sender
                </button>
              {/if}
            </div>
          </div>
        {/if}

        <!-- Body — always rendered in a sandboxed iframe. The sandbox omits
             allow-scripts and allow-same-origin, so message markup cannot run
             code or reach this origin no matter what it contains. -->
        <div class="flex-1 min-h-0 overflow-hidden">
          <iframe
            title="Email content"
            srcdoc={frameDoc}
            sandbox="allow-popups allow-popups-to-escape-sandbox"
            referrerpolicy="no-referrer"
            class="w-full h-full border-0 block"
            class:bg-white={!$darkMode}
            class:bg-gray-800={$darkMode}
          ></iframe>
        </div>

      </div>
    {/key}
  {/if}
</section>
