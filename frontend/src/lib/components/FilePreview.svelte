<script>
  import Modal from './Modal.svelte';
  import Spinner from './Spinner.svelte';
  import { previewNode } from '$lib/stores/files.js';
  import { blobUrl, downloadNode, fetchTextBlob } from '$lib/files.js';
  import { fileKind, formatBytes } from '$lib/fileTypes.js';

  let textContent = '';
  let loadingText = false;
  let textError   = '';

  $: node = $previewNode;
  $: kind = node ? fileKind(node) : 'file';
  $: src  = node ? blobUrl(node, { inline: true }) : '';

  // Text previews are the only kind we have to fetch ourselves; the rest are
  // handled by the browser via a src attribute.
  let loadedFor = null;
  $: if (node && kind === 'text' && node.id !== loadedFor) {
    loadedFor = node.id;
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

  function close() {
    previewNode.set(null);
    textContent = '';
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
    {:else if kind === 'image'}
      <img {src} alt={node.name}
           class="max-w-full max-h-[70vh] object-contain" />
    {:else if kind === 'pdf'}
      <!-- The backend serves this sandboxed and nosniff'd; the iframe adds a
           second layer so a PDF that is really something else cannot act. -->
      <iframe {src} title={node.name} sandbox=""
              class="w-full h-[70vh] border-0 bg-white"></iframe>
    {:else if kind === 'video'}
      <!-- svelte-ignore a11y-media-has-caption -->
      <video {src} controls class="max-w-full max-h-[70vh]"></video>
    {:else if kind === 'audio'}
      <audio {src} controls class="w-full px-8"></audio>
    {:else if kind === 'text'}
      {#if loadingText}
        <div class="flex items-center gap-2 text-sm text-gray-500 dark:text-gray-400 py-12">
          <Spinner size="sm" label="" /> Loading preview…
        </div>
      {:else if textError}
        <p class="text-sm text-red-500 py-12">{textError}</p>
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
    <span class="text-xs text-gray-400 dark:text-gray-500 truncate">
      {node?.modified ? new Date(node.modified).toLocaleString() : ''}
    </span>
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
  </svelte:fragment>
</Modal>
