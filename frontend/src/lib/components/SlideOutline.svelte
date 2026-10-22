<script>
  /**
   * A presentation as it can be shown truthfully without a slide renderer:
   * the cover thumbnail the file carries, then every slide's title and text.
   */
  import { onDestroy } from 'svelte';

  /** @type {Blob|null} a raster image, already checked by its bytes */
  export let thumbnail = null;
  /** @type {{ number: number, title: string, lines: string[], hidden?: boolean }[]} */
  export let slides = [];
  export let truncated = false;

  let url = '';
  $: {
    if (url) URL.revokeObjectURL(url);
    url = thumbnail ? URL.createObjectURL(thumbnail) : '';
  }
  onDestroy(() => { if (url) URL.revokeObjectURL(url); });
</script>

<div class="w-full max-h-[calc(90vh-9.5rem)] overflow-auto bg-white dark:bg-gray-800">
  {#if url}
    <div class="flex justify-center px-6 pt-6">
      <img src={url} alt="Cover of the presentation"
           class="max-h-[40vh] rounded-md border border-gray-200 dark:border-gray-700 shadow-sm" />
    </div>
  {/if}

  <ol class="px-6 py-5 space-y-3">
    {#each slides as slide (slide.number)}
      <li class="flex gap-3">
        <span class="shrink-0 w-7 h-7 rounded-md flex items-center justify-center text-xs font-medium
                     bg-gray-100 dark:bg-gray-700 text-gray-500 dark:text-gray-400">{slide.number}</span>
        <div class="min-w-0 flex-1 pt-0.5">
          <p class="text-sm font-medium text-gray-900 dark:text-gray-100 break-words">
            {slide.title || 'Untitled slide'}
            {#if slide.hidden}
              <span class="ml-1.5 text-[10px] font-normal px-1.5 py-0.5 rounded-sm bg-gray-100 dark:bg-gray-700
                           text-gray-500 dark:text-gray-400">hidden</span>
            {/if}
          </p>
          {#if slide.lines.length}
            <ul class="mt-1 space-y-0.5">
              {#each slide.lines as line}
                <li class="text-xs text-gray-500 dark:text-gray-400 break-words">{line}</li>
              {/each}
            </ul>
          {/if}
        </div>
      </li>
    {/each}
  </ol>

  <p class="px-6 pb-4 text-[11px] text-gray-400 dark:text-gray-500">
    {#if truncated}Only the first slides are listed. {/if}An outline of the text on each slide. Download the file to see the slides as designed.
  </p>
</div>
