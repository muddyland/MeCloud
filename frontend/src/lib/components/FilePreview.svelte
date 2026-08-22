<script>
  import Modal from './Modal.svelte';
  import Spinner from './Spinner.svelte';
  import { previewNode } from '$lib/stores/files.js';
  import { blobUrl, downloadNode, fetchTextBlob, saveTextFile } from '$lib/files.js';
  import { fileKind, formatBytes, isPreviewable } from '$lib/fileTypes.js';
  import { isMarkdown, renderMarkdown } from '$lib/markdown.js';
  import { fileNodes } from '$lib/stores/files.js';
  import { jmapAccountId, jmapSession } from '$lib/stores/mail.js';
  import { toast } from '$lib/stores/toast.js';

  let textContent = '';
  let loadingText = false;
  let textError   = '';

  // Editing, for text and Markdown files. Same save path the Notes app uses —
  // upload a new blob, then repoint the FileNode at it.
  let editing = false;
  let draft   = '';
  let saving  = false;

  $: markdown = node ? isMarkdown(node) : false;
  $: editable = node ? (kind === 'text' || markdown) : false;
  $: renderedMd = markdown && !editing ? renderMarkdown(textContent) : '';

  $: node = $previewNode;
  $: kind = node ? fileKind(node) : 'file';
  // Empty when the node carries no blobId. Every branch below guards on it:
  // `<iframe src="">` loads the *parent* URL rather than nothing, and the app
  // refuses to be framed, so an unguarded empty src surfaces as a confusing
  // "Refused to display ... X-Frame-Options" error naming the app root.
  $: src  = node ? blobUrl(node, { inline: true }) : '';

  // Text previews are the only kind we have to fetch ourselves; the rest are
  // handled by the browser via a src attribute.
  let loadedFor = null;
  $: if (node && kind === 'text' && node.id !== loadedFor) {
    loadedFor = node.id;
    editing = false;
    draft = '';
    loadText(node);
  }

  async function loadText(target) {
    loadingText = true;
    textError   = '';
    textContent = '';
    try {
      const text = await fetchTextBlob(target);
      if ($previewNode?.id === target.id) textContent = text;
    } catch (e) {
      textError = e?.message ?? 'Could not load a preview of this file.';
    } finally {
      loadingText = false;
    }
  }

  function startEdit() {
    draft = textContent;
    editing = true;
  }

  function cancelEdit() {
    editing = false;
    draft = '';
  }

  async function saveEdit() {
    if (!node || saving) return;
    saving = true;
    try {
      const updated = await saveTextFile($jmapAccountId, $jmapSession, node, draft);
      textContent = draft;
      // Keep the Files listing's size/modified in step with what was just written.
      fileNodes.update((list) => list.map((n) => (n.id === node.id ? { ...n, ...updated } : n)));
      previewNode.set({ ...node, ...updated });
      editing = false;
      toast('Saved', 'success');
    } catch (e) {
      toast(e?.message ?? 'Could not save this file.', 'error');
    } finally {
      saving = false;
    }
  }

  function close() {
    // Closing mid-edit would silently discard the change.
    if (editing && draft !== textContent) {
      if (!confirm('Discard unsaved changes to this file?')) return;
    }
    previewNode.set(null);
    textContent = '';
    draft = '';
    editing = false;
    loadedFor = null;
  }
</script>

<Modal
  open={!!node}
  title={node?.name ?? ''}
  subtitle={node ? `${formatBytes(node.size)}${node.type ? ` · ${node.type}` : ''}` : ''}
  size="xl"
  on:close={close}
>
  <div class="flex items-center justify-center min-h-[16rem] bg-gray-50 dark:bg-gray-900/50">
    {#if !node}
      <!-- closed -->
    {:else if kind === 'image' && src}
      <img {src} alt={node.name}
           class="max-w-full max-h-[70vh] object-contain" />
    {:else if kind === 'pdf' && src}
      <!-- Deliberately not sandbox="": an empty sandbox disables plugins, and
           the browser's PDF viewer is one, so the frame rendered as "This
           content is blocked". Safety comes from the response instead — the
           backend only ever serves inline types from a fixed allow-list that
           excludes everything script-capable, with nosniff and a restrictive
           per-response CSP. -->
      <iframe {src} title={node.name} referrerpolicy="no-referrer"
              class="w-full h-[70vh] border-0 bg-white"></iframe>
    {:else if kind === 'video' && src}
      <!-- svelte-ignore a11y-media-has-caption -->
      <video {src} controls class="max-w-full max-h-[70vh]"></video>
    {:else if kind === 'audio' && src}
      <audio {src} controls class="w-full px-8"></audio>
    {:else if kind === 'text'}
      {#if loadingText}
        <div class="flex items-center gap-2 text-sm text-gray-500 dark:text-gray-400 py-12">
          <Spinner size="sm" label="" /> Loading preview…
        </div>
      {:else if textError}
        <p class="text-sm text-red-500 py-12">{textError}</p>
      {:else if editing}
        <textarea
          bind:value={draft}
          spellcheck={markdown}
          class="w-full h-[60vh] resize-none p-4 font-mono text-xs leading-relaxed
                 bg-white dark:bg-gray-800 text-gray-800 dark:text-gray-200
                 focus:outline-none"
        ></textarea>
      {:else if markdown}
        <!-- Sanitised in renderMarkdown(); this renders in the parent document,
             so that call is the only boundary. -->
        <div class="w-full max-h-[70vh] overflow-auto p-5 bg-white dark:bg-gray-800">
          <article class="note-prose">{@html renderedMd}</article>
        </div>
      {:else}
        <pre class="w-full max-h-[70vh] overflow-auto p-4 text-xs leading-relaxed
                    font-mono text-gray-800 dark:text-gray-200 whitespace-pre-wrap
                    break-words">{textContent}</pre>
      {/if}
    {:else}
      <div class="flex flex-col items-center gap-3 py-16 text-gray-400 dark:text-gray-500">
        <svg class="w-12 h-12 opacity-50" viewBox="0 0 24 24" fill="none" stroke="currentColor"
             stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round">
          <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" /><path d="M14 3v5h5" />
        </svg>
        <p class="text-sm">No preview available for this file type.</p>
      </div>
    {/if}
  </div>

  <svelte:fragment slot="footer">
    <span class="text-xs text-gray-400 dark:text-gray-500 truncate min-w-0">
      {node?.modified ? new Date(node.modified).toLocaleString() : ''}
    </span>
    <div class="flex items-center gap-1 flex-shrink-0">
    {#if editable && !loadingText && !textError}
      {#if editing}
        <button
          on:click={saveEdit}
          disabled={saving}
          class="inline-flex items-center gap-2 px-3 py-1.5 text-sm font-medium rounded-lg
                 bg-blue-500 hover:bg-blue-600 text-white disabled:opacity-60
                 transition-colors duration-150"
        >
          {#if saving}
            <Spinner size="xs" label="" accent="border-t-white" cls="border-white/40" />
            Saving…
          {:else}
            Save
          {/if}
        </button>
        <button on:click={cancelEdit} disabled={saving}
          class="px-3 py-1.5 text-sm text-gray-500 dark:text-gray-400
                 hover:text-gray-700 dark:hover:text-gray-200 disabled:opacity-40
                 transition-colors duration-150">
          Cancel
        </button>
      {:else}
        <button
          on:click={startEdit}
          class="inline-flex items-center gap-2 px-3 py-1.5 text-sm font-medium rounded-lg
                 text-gray-600 dark:text-gray-300
                 hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors duration-150"
        >
          <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor"
               stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 20h9M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4z" />
          </svg>
          Edit
        </button>
      {/if}
    {/if}

    {#if node && isPreviewable(node) && !editing}
      <a
        href={src}
        target="_blank"
        rel="noopener noreferrer"
        class="inline-flex items-center gap-2 px-3 py-1.5 text-sm font-medium rounded-lg
               text-gray-600 dark:text-gray-300
               hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors duration-150"
      >
        <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor"
             stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
          <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
          <path d="M15 3h6v6M10 14 21 3" />
        </svg>
        Open in new tab
      </a>
    {/if}
    <button
      on:click={() => node && downloadNode(node)}
      class="inline-flex items-center gap-2 px-3 py-1.5 text-sm font-medium rounded-lg
             bg-blue-500 hover:bg-blue-600 text-white transition-colors duration-150"
    >
      <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor"
           stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 3v12m0 0-4-4m4 4 4-4M5 19h14" />
      </svg>
      Download
    </button>
    </div>
  </svelte:fragment>
</Modal>
