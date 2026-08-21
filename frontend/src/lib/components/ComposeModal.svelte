<script>
  import { fly } from 'svelte/transition';
  import { composeOpen, composeContext, jmapAccountId, mailboxes, currentUser } from '$lib/stores/mail.js';
  import { getIdentities, sendEmail } from '$lib/api.js';
  import { toast } from '$lib/stores/toast.js';
  import { sanitizeEmailHtml } from '$lib/sanitize.js';
  import Spinner from './Spinner.svelte';

  let to = '', cc = '', bcc = '', subject = '';
  let showCc  = false;
  let showBcc = false;
  let editorEl;
  let initialBody = '';       // what the editor held before the user touched it
  let confirmingDiscard = false;

  // ── Identity ──────────────────────────────────────────────────────────────
  let identities        = [];
  let selectedIdentityId = '';
  let identitiesLoaded  = false;

  $: if ($composeOpen && $jmapAccountId && !identitiesLoaded) {
    loadIdentities();
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
    if ($composeContext) {
      to      = $composeContext.to      ?? '';
      cc      = $composeContext.cc      ?? '';
      subject = $composeContext.subject ?? '';
      showCc  = !!$composeContext.cc;
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
             overflow-hidden max-h-[92vh] sm:max-h-[90vh]"
      role="dialog"
      aria-label={title}
      on:click|stopPropagation
    >
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
{/if}

<style>
  [contenteditable]:empty:before {
    content: attr(data-placeholder);
    color: #9ca3af;
    pointer-events: none;
  }
</style>
