<script>
  import { fly } from 'svelte/transition';
  import { composeOpen, composeContext, jmapAccountId, jmapSession, mailboxes, currentUser } from '$lib/stores/mail.js';
  import { getIdentities, getMailboxes, sendEmail } from '$lib/api.js';
  import { toast } from '$lib/stores/toast.js';
  import { sanitizeEmailHtml } from '$lib/sanitize.js';
  import { uploadBlob, supportsFiles } from '$lib/files.js';
  import { formatBytes } from '$lib/fileTypes.js';
  import { selectDriveAttachments } from '$lib/attachments.js';
  import Spinner from './Spinner.svelte';
  import FilePicker from './FilePicker.svelte';

  let to = '', cc = '', bcc = '', subject = '';
  let showCc  = false;
  let showBcc = false;
  let editorEl;
  let initialBody = '';       // what the editor held before the user touched it
  let confirmingDiscard = false;
  let attachInput;
  let pickerOpen = false;
  let dragDepth = 0;          // counter, not a boolean — dragleave fires per child

  // ── Attachments ───────────────────────────────────────────────────────────
  //
  // Bytes go up first, on their own, and the message only ever carries blob
  // ids. That is what JMAP asks for, and it also means a file already in the
  // drive is attached by reference — "share this via email" costs no transfer
  // at all rather than a download followed by a re-upload.

  /**
   * Practically every receiving server rejects a message past ~25 MB, and it
   * does so *after* the upload, so the limit is enforced here where the user
   * can still do something about it.
   */
  const MAX_TOTAL_BYTES = 25 * 1024 * 1024;

  /** { id, name, type, size, blobId, progress, error, uploading, controller } */
  let attachments = [];
  let attachSeq = 0;
  let attachError = '';

  $: attachedBytes = attachments.reduce((sum, a) => sum + (Number(a.size) || 0), 0);
  $: uploadingCount = attachments.filter((a) => a.uploading).length;

  const patchAttachment = (id, fields) => {
    attachments = attachments.map((a) => (a.id === id ? { ...a, ...fields } : a));
  };

  function addFiles(fileList) {
    const files = [...(fileList ?? [])];
    if (!files.length) return;

    let running = attachedBytes;
    for (const file of files) {
      if (running + file.size > MAX_TOTAL_BYTES) {
        attachError =
          `Attachments are limited to ${formatBytes(MAX_TOTAL_BYTES)} in total — `
          + `"${file.name}" was not attached.`;
        continue;
      }
      running += file.size;
      attachOne(file);
    }
  }

  async function attachOne(file) {
    const id = ++attachSeq;
    const controller = new AbortController();
    attachments = [...attachments, {
      id,
      name: file.name,
      type: file.type || 'application/octet-stream',
      size: file.size,
      blobId: null,
      progress: 0,
      error: '',
      uploading: true,
      controller,
    }];

    try {
      const blob = await uploadBlob(file, {
        signal: controller.signal,
        onProgress: (p) => patchAttachment(id, { progress: p }),
      });
      patchAttachment(id, {
        blobId: blob.blobId,
        type: blob.type || file.type || 'application/octet-stream',
        size: blob.size ?? file.size,
        progress: 1,
        uploading: false,
        controller: null,
      });
    } catch (e) {
      // A removal aborts the request; that is not a failure worth reporting.
      if (e?.name === 'AbortError') return;
      patchAttachment(id, {
        error: e?.message ?? 'Upload failed.',
        uploading: false,
        controller: null,
      });
    }
  }

  // ── From the drive ────────────────────────────────────────────────────────
  //
  // These are already stored in the account, so there is nothing to upload:
  // the row is born complete and the message carries the existing blob id.

  $: canAttachFromFiles = supportsFiles($jmapSession);
  $: attachedBlobIds = new Set(attachments.map((a) => a.blobId).filter(Boolean));

  function attachFromFiles(nodes) {
    pickerOpen = false;
    const { rows, skipped } = selectDriveAttachments(nodes, {
      attachedBlobIds,
      usedBytes: attachedBytes,
      maxTotalBytes: MAX_TOTAL_BYTES,
    });

    if (rows.length) {
      attachments = [...attachments, ...rows.map((row) => ({
        ...row,
        id: ++attachSeq,
        progress: 1,
        error: '',
        uploading: false,
        controller: null,
      }))];
    }
    // A duplicate is silent — the file the user wanted attached is attached,
    // which is what they asked for. Running out of budget is not.
    attachError = skipped
      ? `Attachments are limited to ${formatBytes(MAX_TOTAL_BYTES)} in total — `
        + `${skipped} file${skipped === 1 ? ' was' : 's were'} not attached.`
      : '';
  }

  function removeAttachment(id) {
    const row = attachments.find((a) => a.id === id);
    row?.controller?.abort();
    attachments = attachments.filter((a) => a.id !== id);
    attachError = '';
  }

  function clearAttachments() {
    for (const row of attachments) row.controller?.abort();
    attachments = [];
    attachError = '';
  }

  function onAttachPicked(event) {
    const input = event.currentTarget;
    addFiles(input.files);
    input.value = '';                      // allow re-picking the same file
  }

  function onDragEnter(e) {
    if (!e.dataTransfer?.types?.includes('Files')) return;
    dragDepth += 1;
  }
  function onDragLeave() { dragDepth = Math.max(0, dragDepth - 1); }
  function onDrop(e) {
    if (!e.dataTransfer?.types?.includes('Files')) return;
    e.preventDefault();
    dragDepth = 0;
    addFiles(e.dataTransfer.files);
  }

  // ── Identity ──────────────────────────────────────────────────────────────
  let identities        = [];
  let selectedIdentityId = '';
  let identitiesLoaded  = false;

  $: if ($composeOpen && $jmapAccountId && !identitiesLoaded) {
    loadIdentities();
  }

  // Compose is reachable from Files and Contacts, which never load the mailbox
  // list — and without it there is no Sent folder to file the copy into, so the
  // message goes out and leaves no trace in the account.
  let mailboxesRequested = false;
  $: if ($composeOpen && $jmapAccountId && !$mailboxes.length && !mailboxesRequested) {
    mailboxesRequested = true;
    getMailboxes($jmapAccountId)
      .then((list) => mailboxes.set(list))
      .catch(() => { /* Sending still works; only the Sent copy is lost. */ });
  }

  async function loadIdentities() {
    identitiesLoaded = true;
    try {
      identities = await getIdentities($jmapAccountId);
      if (identities.length > 0) {
        const primary = identities.find(i => i.mayDelete === false)
          ?? identities.find(i => i.email === $currentUser)
          ?? identities[0];
        selectedIdentityId = primary.id;
      }
    } catch {
      identities = [{ id: 'fallback', email: $currentUser, name: '' }];
      selectedIdentityId = 'fallback';
    }
  }

  $: selectedIdentity = identities.find(i => i.id === selectedIdentityId) ?? null;

  // ── Compose mode ──────────────────────────────────────────────────────────
  $: mode  = $composeContext?.mode ?? 'compose';
  $: title = mode === 'reply'     ? 'Reply'
           : mode === 'reply_all' ? 'Reply All'
           : mode === 'forward'   ? 'Forward'
           : 'New Message';

  // Pre-fill fields when context changes
  let appliedContext = null;
  $: if ($composeContext !== appliedContext) {
    appliedContext = $composeContext;
    to = cc = bcc = subject = '';
    showCc = showBcc = false;
    confirmingDiscard = false;
    clearAttachments();
    if ($composeContext) {
      to      = $composeContext.to      ?? '';
      cc      = $composeContext.cc      ?? '';
      subject = $composeContext.subject ?? '';
      showCc  = !!$composeContext.cc;
      // Pre-attached blobs — a file shared from the drive. They are already
      // uploaded, so they arrive complete rather than as an upload row.
      attachments = ($composeContext.attachments ?? [])
        .filter((a) => a?.blobId)
        .map((a) => ({
          id: ++attachSeq,
          name: a.name ?? 'attachment',
          type: a.type || 'application/octet-stream',
          size: Number(a.size) || 0,
          blobId: a.blobId,
          progress: 1,
          error: '',
          uploading: false,
          controller: null,
        }));
      // body is set via use:initBody on the contenteditable when it mounts
    }
  }

  // ── Send state ────────────────────────────────────────────────────────────
  let sending   = false;
  let sendError = '';

  /** Anything the user typed that isn't part of the pre-filled reply. */
  function isDirty() {
    const prefill = $composeContext ?? {};
    if (to.trim() !== (prefill.to ?? '').trim()) return true;
    if (cc.trim() !== (prefill.cc ?? '').trim()) return true;
    if (bcc.trim()) return true;
    if (subject.trim() !== (prefill.subject ?? '').trim()) return true;
    // Anything attached beyond what was handed in is the user's own work, and
    // an upload they waited for is the last thing to throw away silently.
    if (attachments.length !== (prefill.attachments?.length ?? 0)) return true;
    return (editorEl?.innerHTML ?? '').trim() !== initialBody;
  }

  function reset() {
    composeOpen.set(false);
    composeContext.set(null);
    appliedContext = null;
    to = ''; cc = ''; bcc = ''; subject = '';
    showCc = showBcc = false;
    sendError = '';
    confirmingDiscard = false;
    initialBody = '';
    dragDepth = 0;
    pickerOpen = false;
    clearAttachments();
    if (editorEl) editorEl.innerHTML = '';
  }

  /**
   * Losing a half-written message to a stray backdrop click is the classic
   * webmail papercut, so an unsent draft asks before it goes.
   */
  function close() {
    if (sending) return;
    if (isDirty() && !confirmingDiscard) {
      confirmingDiscard = true;
      return;
    }
    reset();
  }

  function onKeydown(e) {
    if (!$composeOpen) return;
    // The picker is a modal in front of this one and handles its own Escape;
    // without this, one press would close both — and prompt to discard the draft.
    if (pickerOpen) return;
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
    } else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      send();
    }
  }

  async function send() {
    if (sending) return;

    const html = editorEl?.innerHTML?.trim() ?? '';
    if (!to.trim() && mode !== 'forward') {
      sendError = 'Please enter at least one recipient.';
      return;
    }
    // Sending now would drop the parts still in flight — silently, since the
    // message would go out looking perfectly normal minus its attachments.
    if (uploadingCount > 0) {
      sendError = `Waiting for ${uploadingCount} attachment${uploadingCount === 1 ? '' : 's'} to finish uploading.`;
      return;
    }
    const failed = attachments.filter((a) => a.error);
    if (failed.length) {
      sendError = `${failed.length} attachment${failed.length === 1 ? ' did' : 's did'} not upload. Remove or retry ${failed.length === 1 ? 'it' : 'them'} first.`;
      return;
    }

    sending   = true;
    sendError = '';

    const sentMailbox = $mailboxes.find(m => m.role === 'sent');

    try {
      await sendEmail($jmapAccountId, selectedIdentityId, {
        fromEmail: selectedIdentity?.email ?? $currentUser,
        fromName:  selectedIdentity?.name  ?? '',
        to,
        cc,
        bcc,
        subject,
        html,
        sentMailboxId: sentMailbox?.id ?? null,
        inReplyTo:  $composeContext?.inReplyTo  ?? null,
        references: $composeContext?.references ?? null,
        attachments,
      });
      sending = false;
      toast('Message sent', 'success');
      reset();
    } catch (e) {
      sendError = e?.message ?? 'Failed to send. Please try again.';
      sending = false;
    }
  }

  // ── Body init action ──────────────────────────────────────────────────────
  // Explicit parameter avoids any store-subscription timing ambiguity.
  //
  // The quoted reply/forward block is assembled from message headers, and this
  // is the one place where message-derived markup lands in the *parent*
  // document rather than the sandboxed iframe. It is escaped at the source, but
  // it gets sanitised again here so a future change upstream cannot quietly
  // turn a header into script. allowRemote is true because anything the user
  // was not meant to see has already been stripped by this point.
  function initBody(node, body) {
    node.innerHTML = sanitizeEmailHtml(body ?? '', { allowRemote: true }).html;
    // Read back what the DOM actually stored: the browser normalises markup on
    // assignment, so comparing against the string we passed in would report a
    // pristine reply as edited and prompt to discard on every close.
    initialBody = node.innerHTML.trim();
    return { update() {}, destroy() {} };
  }

  // ── Rich text ─────────────────────────────────────────────────────────────
  function fmt(cmd, value = null) {
    document.execCommand(cmd, false, value);
    editorEl?.focus();
  }

  function insertLink() {
    const url = prompt('URL:');
    if (url) fmt('createLink', url);
  }

  function insertImage() {
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = 'image/*';
    input.onchange = () => {
      const file = input.files?.[0];
      if (!file) return;
      const reader = new FileReader();
      reader.onload = (e) => fmt('insertImage', e.target.result);
      reader.readAsDataURL(file);
    };
    input.click();
  }

  const tbtn = `p-1.5 rounded text-gray-500 dark:text-gray-400
    hover:bg-gray-100 dark:hover:bg-gray-700
    hover:text-gray-800 dark:hover:text-gray-200
    transition-colors duration-100 text-sm leading-none`;

  const field = `flex-1 py-2.5 text-sm bg-transparent text-gray-800 dark:text-gray-100
    placeholder-gray-400 dark:placeholder-gray-500 outline-none`;

  const label = `text-xs text-gray-400 dark:text-gray-500 w-16 flex-shrink-0 select-none`;
</script>

<svelte:window on:keydown={onKeydown} />

{#if $composeOpen}
  <div
    class="fixed inset-0 z-50 flex items-end sm:items-center justify-center
           bg-gray-900/40 dark:bg-black/60 backdrop-blur-[2px] p-0 sm:p-4"
    on:click={close}
    role="presentation"
  >
    <div
      transition:fly={{ y: 16, duration: 200 }}
      class="w-full max-w-2xl flex flex-col rounded-t-xl sm:rounded-xl shadow-2xl
             pb-safe sm:pb-0
             bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-700
             overflow-hidden max-h-[92vh] sm:max-h-[90vh] relative"
      role="dialog"
      aria-label={title}
      on:click|stopPropagation
      on:dragenter={onDragEnter}
      on:dragover|preventDefault
      on:dragleave={onDragLeave}
      on:drop={onDrop}
    >
      <!-- Drop-to-attach overlay -->
      {#if dragDepth > 0}
        <div class="absolute inset-0 z-10 flex items-center justify-center pointer-events-none
                    bg-blue-500/10 border-2 border-dashed border-blue-400 dark:border-blue-500
                    m-2 rounded-lg">
          <div class="flex flex-col items-center gap-2 text-blue-600 dark:text-blue-300">
            <svg class="w-8 h-8" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
              <path d="M21.44 11.05l-9.19 9.19a6 6 0 01-8.49-8.49l9.19-9.19a4 4 0 015.66 5.66l-9.2 9.19a2 2 0 01-2.83-2.83l8.49-8.48"/>
            </svg>
            <p class="text-sm font-medium">Drop to attach</p>
          </div>
        </div>
      {/if}

      <!-- Header -->
      <div class="flex items-center justify-between px-4 py-3 border-b border-gray-200 dark:border-gray-700 flex-shrink-0">
        <span class="text-sm font-semibold text-gray-800 dark:text-gray-100">{title}</span>
        <button on:click={close}
          class="text-gray-400 hover:text-gray-600 dark:hover:text-gray-200 text-xl leading-none"
        >×</button>
      </div>

      <!-- Address fields -->
      <div class="flex flex-col divide-y divide-gray-100 dark:divide-gray-700 border-b border-gray-200 dark:border-gray-700 flex-shrink-0">

        <!-- From -->
        {#if identities.length > 1}
          <div class="flex items-center px-4 gap-2">
            <span class={label}>From</span>
            <select bind:value={selectedIdentityId} class="flex-1 py-2.5 text-sm bg-transparent
              text-gray-800 dark:text-gray-100 outline-none">
              {#each identities as id}
                <option value={id.id}>{id.name ? `${id.name} <${id.email}>` : id.email}</option>
              {/each}
            </select>
          </div>
        {:else if selectedIdentity}
          <div class="flex items-center px-4 gap-2">
            <span class={label}>From</span>
            <span class="py-2.5 text-sm text-gray-500 dark:text-gray-400">
              {selectedIdentity.name ? `${selectedIdentity.name} <${selectedIdentity.email}>` : selectedIdentity.email}
            </span>
          </div>
        {/if}

        <!-- To -->
        <div class="flex items-center px-4 gap-2">
          <span class={label}>To</span>
          <!-- svelte-ignore a11y-autofocus -->
          <input bind:value={to} placeholder="Recipients" type="text" class={field}
            autocomplete="off" autofocus={!$composeContext?.to} />
          <div class="flex items-center gap-1 flex-shrink-0">
            {#if !showCc}
              <button on:click={() => (showCc = true)}
                class="text-xs text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 px-1">
                Cc
              </button>
            {/if}
            {#if !showBcc}
              <button on:click={() => (showBcc = true)}
                class="text-xs text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 px-1">
                Bcc
              </button>
            {/if}
          </div>
        </div>

        {#if showCc}
          <div class="flex items-center px-4 gap-2">
            <span class={label}>Cc</span>
            <input bind:value={cc} placeholder="Cc recipients" type="text" class={field}
              autocomplete="off" />
          </div>
        {/if}

        {#if showBcc}
          <div class="flex items-center px-4 gap-2">
            <span class={label}>Bcc</span>
            <input bind:value={bcc} placeholder="Bcc recipients" type="text" class={field}
              autocomplete="off" />
          </div>
        {/if}

        <!-- Subject -->
        <div class="flex items-center px-4 gap-2">
          <span class={label}>Subject</span>
          <input bind:value={subject} placeholder="Subject" type="text" class={field} />
        </div>
      </div>

      <!-- Formatting toolbar -->
      <div class="flex items-center gap-0.5 px-3 py-1.5 border-b border-gray-100 dark:border-gray-700 flex-shrink-0 flex-wrap">
        <button on:click={() => fmt('bold')}          class="{tbtn} font-bold" title="Bold">B</button>
        <button on:click={() => fmt('italic')}        class="{tbtn} italic"    title="Italic">I</button>
        <button on:click={() => fmt('underline')}     class="{tbtn} underline" title="Underline">U</button>
        <button on:click={() => fmt('strikeThrough')} class="{tbtn} line-through" title="Strikethrough">S</button>

        <div class="w-px h-4 bg-gray-200 dark:bg-gray-600 mx-1"></div>

        <button on:click={() => fmt('insertUnorderedList')} class={tbtn} title="Bullet list">
          <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <line x1="9" y1="6" x2="20" y2="6"/><line x1="9" y1="12" x2="20" y2="12"/><line x1="9" y1="18" x2="20" y2="18"/>
            <circle cx="4" cy="6" r="1.5" fill="currentColor" stroke="none"/>
            <circle cx="4" cy="12" r="1.5" fill="currentColor" stroke="none"/>
            <circle cx="4" cy="18" r="1.5" fill="currentColor" stroke="none"/>
          </svg>
        </button>
        <button on:click={() => fmt('insertOrderedList')} class={tbtn} title="Numbered list">
          <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <line x1="10" y1="6" x2="21" y2="6"/><line x1="10" y1="12" x2="21" y2="12"/><line x1="10" y1="18" x2="21" y2="18"/>
            <path d="M4 6h1v4M4 10h2" stroke-linecap="round"/>
            <path d="M6 18H4c0-1 2-2 2-3s-1-1-2-1" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
        </button>

        <div class="w-px h-4 bg-gray-200 dark:bg-gray-600 mx-1"></div>

        <button on:click={insertLink} class={tbtn} title="Insert link">
          <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M10 13a5 5 0 007.54.54l3-3a5 5 0 00-7.07-7.07l-1.72 1.71"/>
            <path d="M14 11a5 5 0 00-7.54-.54l-3 3a5 5 0 007.07 7.07l1.71-1.71"/>
          </svg>
        </button>
        <button on:click={insertImage} class={tbtn} title="Insert image">
          <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <rect x="3" y="3" width="18" height="18" rx="2"/>
            <circle cx="8.5" cy="8.5" r="1.5"/>
            <polyline points="21 15 16 10 5 21"/>
          </svg>
        </button>
        <button on:click={() => attachInput?.click()} class={tbtn}
                title="Attach a file from this device">
          <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <path d="M21.44 11.05l-9.19 9.19a6 6 0 01-8.49-8.49l9.19-9.19a4 4 0 015.66 5.66l-9.2 9.19a2 2 0 01-2.83-2.83l8.49-8.48"/>
          </svg>
        </button>
        {#if canAttachFromFiles}
          <button on:click={() => (pickerOpen = true)} class={tbtn}
                  title="Attach from Files">
            <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                 stroke-linecap="round" stroke-linejoin="round">
              <path d="M3 7a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>
              <path d="M12 17v-5m0 0-2 2m2-2 2 2"/>
            </svg>
          </button>
        {/if}

        <div class="w-px h-4 bg-gray-200 dark:bg-gray-600 mx-1"></div>

        <button on:click={() => fmt('removeFormat')} class={tbtn} title="Clear formatting">
          <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M6 12h8M12 6l4 6-4 6"/><line x1="4" y1="20" x2="20" y2="4"/>
          </svg>
        </button>
      </div>

      <!-- Editable body -->
      <div
        bind:this={editorEl}
        use:initBody={$composeContext?.body ?? ''}
        contenteditable="true"
        class="flex-1 overflow-y-auto px-4 py-3 text-sm text-gray-800 dark:text-gray-100
               outline-none leading-relaxed"
        style="min-height: 280px"
        data-placeholder="Write your message…"
      ></div>

      <input bind:this={attachInput} type="file" multiple class="hidden"
             on:change={onAttachPicked} />

      <!-- Attachments -->
      {#if attachments.length || attachError}
        <div class="px-4 py-2 border-t border-gray-100 dark:border-gray-700 flex-shrink-0
                    max-h-40 overflow-y-auto">
          {#if attachError}
            <p class="text-xs text-red-500 mb-2">{attachError}</p>
          {/if}
          {#if attachments.length}
            <div class="flex items-center gap-2 mb-1.5 text-xs text-gray-400 dark:text-gray-500">
              <span>
                {attachments.length} attachment{attachments.length === 1 ? '' : 's'}
                · {formatBytes(attachedBytes)}
              </span>
            </div>
            <div class="flex flex-wrap gap-2">
              {#each attachments as att (att.id)}
                <div
                  class="inline-flex items-center gap-2 max-w-full sm:max-w-[16rem] pl-2 pr-1 py-1 rounded-md
                         text-xs transition-colors duration-150
                         {att.error
                           ? 'bg-red-50 dark:bg-red-900/25 text-red-700 dark:text-red-300'
                           : 'bg-gray-100 dark:bg-gray-700/60 text-gray-600 dark:text-gray-300'}"
                  title={att.error || att.name}
                >
                  {#if att.uploading}
                    <Spinner size="xs" label="" />
                  {:else if att.error}
                    <svg class="w-3.5 h-3.5 flex-shrink-0" viewBox="0 0 24 24" fill="none"
                         stroke="currentColor" stroke-width="2" stroke-linecap="round">
                      <circle cx="12" cy="12" r="9" /><path d="M12 8v5M12 16h.01" />
                    </svg>
                  {:else}
                    <svg class="w-3.5 h-3.5 flex-shrink-0" viewBox="0 0 24 24" fill="none"
                         stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
                      <path d="M21.44 11.05l-9.19 9.19a6 6 0 01-8.49-8.49l9.19-9.19a4 4 0 015.66 5.66l-9.2 9.19a2 2 0 01-2.83-2.83l8.49-8.48"/>
                    </svg>
                  {/if}

                  <span class="truncate">{att.name}</span>

                  {#if att.uploading}
                    <span class="tabular-nums text-gray-400 dark:text-gray-500 flex-shrink-0">
                      {Math.round(att.progress * 100)}%
                    </span>
                  {:else if !att.error && att.size}
                    <span class="text-gray-400 dark:text-gray-500 flex-shrink-0">
                      {formatBytes(att.size)}
                    </span>
                  {/if}

                  <button
                    on:click={() => removeAttachment(att.id)}
                    aria-label="Remove {att.name}"
                    class="p-0.5 rounded flex-shrink-0 text-gray-400 hover:text-gray-700
                           dark:hover:text-gray-100 hover:bg-gray-200 dark:hover:bg-gray-600
                           transition-colors duration-100"
                  >
                    <svg class="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2.5" stroke-linecap="round">
                      <path d="M6 6l12 12M18 6L6 18" />
                    </svg>
                  </button>
                </div>
              {/each}
            </div>
          {/if}
        </div>
      {/if}

      <!-- Footer -->
      <div class="flex items-center justify-between px-4 py-3 border-t border-gray-200 dark:border-gray-700 flex-shrink-0 gap-4">
        <div class="flex items-center gap-3 min-w-0">
          {#if confirmingDiscard}
            <span class="text-xs text-gray-600 dark:text-gray-300">Discard this draft?</span>
            <button on:click={reset}
              class="text-xs font-medium text-red-600 dark:text-red-400 hover:underline">
              Discard
            </button>
            <button on:click={() => (confirmingDiscard = false)}
              class="text-xs text-gray-500 dark:text-gray-400 hover:underline">
              Keep editing
            </button>
          {:else}
            <button on:click={close} disabled={sending}
              class="text-sm text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-200
                     transition-colors duration-150 disabled:opacity-40">
              Discard
            </button>
          {/if}
          {#if sendError}
            <p class="text-xs text-red-500 truncate">{sendError}</p>
          {/if}
        </div>

        <button on:click={send} disabled={sending}
          title="Send (⌘/Ctrl + Enter)"
          class="flex items-center gap-2 px-5 py-2 text-sm font-medium rounded-lg
                 bg-blue-500 hover:bg-blue-600 dark:bg-blue-600 dark:hover:bg-blue-500
                 text-white transition-colors duration-150 flex-shrink-0
                 disabled:opacity-60 disabled:cursor-not-allowed">
          {#if sending}
            <Spinner size="sm" label="" accent="border-t-white" cls="border-white/40" />
            Sending…
          {:else}
            Send
          {/if}
        </button>
      </div>
    </div>
  </div>

  <FilePicker
    open={pickerOpen}
    {attachedBlobIds}
    on:close={() => (pickerOpen = false)}
    on:attach={(e) => attachFromFiles(e.detail)}
  />
{/if}

<style>
  [contenteditable]:empty:before {
    content: attr(data-placeholder);
    color: #9ca3af;
    pointer-events: none;
  }
</style>
