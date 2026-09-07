<script>
  /**
   * An image file's own contents as its listing icon, falling back to the
   * kind icon for everything else.
   *
   * The fallback is not only for non-images: a thumbnail that fails to load —
   * a blob the server no longer has, a type it declines to serve inline —
   * would otherwise leave a broken-image glyph in the grid, which reads as a
   * corrupted file rather than a missing preview.
   */
  import FileIcon from './FileIcon.svelte';
  import { blobUrl } from '$lib/files.js';
  import { canThumbnail } from '$lib/fileTypes.js';

  export let node = null;
  /** sm (list rows) | lg (grid tiles) */
  export let size = 'sm';

  let failed = false;

  // Reset per node, so recycling a tile for a different file re-tries.
  let triedFor = null;
  $: if (node?.id !== triedFor) {
    triedFor = node?.id ?? null;
    failed = false;
  }

  $: showImage = !failed && canThumbnail(node);
  $: src = showImage ? blobUrl(node, { inline: true }) : '';

  const BOX = { sm: 'w-5 h-5', lg: 'w-12 h-12' };
</script>

{#if showImage && src}
  <img
    {src}
    alt=""
    loading="lazy"
    decoding="async"
    draggable="false"
    on:error={() => (failed = true)}
    class="{BOX[size] ?? BOX.sm} flex-shrink-0 object-cover rounded
           bg-gray-100 dark:bg-gray-700 ring-1 ring-black/5 dark:ring-white/10"
  />
{:else}
  <FileIcon {node} {size} />
{/if}
