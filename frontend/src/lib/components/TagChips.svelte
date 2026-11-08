<script>
  /**
   * A file's tags as small chips. Each chip is a button that searches the
   * drive for that tag, which is the whole point of having tagged it.
   *
   * `max` keeps tight layouts (grid tiles) to a couple of chips and a "+N";
   * the full list is one right-click away in Tags…
   */
  /** @type {string[]} */
  export let tags = [];
  /** @type {number|null} */
  export let max = null;
  /** @type {(tag: string) => void} */
  export let onSelect = () => {};
  export let cls = '';

  $: shown = max !== null && tags.length > max ? tags.slice(0, max) : tags;
  $: hidden = tags.length - shown.length;
</script>

{#if tags.length}
  <span class="flex flex-wrap items-center gap-1 min-w-0 {cls}">
    {#each shown as tag (tag)}
      <button
        type="button"
        on:click|stopPropagation={() => onSelect(tag)}
        on:dblclick|stopPropagation
        on:keydown|stopPropagation
        title="Show everything tagged “{tag}”"
        class="max-w-[9rem] truncate px-1.5 py-px rounded-full text-[11px] leading-4
               bg-blue-50 dark:bg-blue-900/40 text-blue-700 dark:text-blue-300
               hover:bg-blue-100 dark:hover:bg-blue-900/70
               focus:outline-hidden focus-visible:ring-2 focus-visible:ring-blue-500
               transition-colors duration-100"
      >{tag}</button>
    {/each}
    {#if hidden > 0}
      <span class="text-[11px] leading-4 text-gray-400 dark:text-gray-500 select-none"
            title={tags.slice(shown.length).join(', ')}>+{hidden}</span>
    {/if}
  </span>
{/if}
