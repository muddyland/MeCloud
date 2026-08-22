<script>
  import { tick } from 'svelte';
  import { applyAction } from '$lib/markdownEdit.js';

  /** The textarea being edited. Bound by the parent. */
  export let textarea = null;
  /** The document text. Two-way bound so the parent's autosave sees edits. */
  export let value = '';
  /** Compact drops the labels — used in the Files preview's smaller footprint. */
  export let compact = false;

  /**
   * Apply an action to the current selection, then put the caret back.
   *
   * The selection has to be restored after Svelte has written the new value to
   * the DOM, otherwise the browser collapses it to the end of the textarea and
   * chaining two formats in a row becomes impossible.
   */
  async function run(action) {
    if (!textarea) return;
    const result = applyAction(action, {
      text: value,
      start: textarea.selectionStart ?? value.length,
      end: textarea.selectionEnd ?? value.length,
    });

    value = result.text;
    await tick();
    textarea.focus();
    textarea.setSelectionRange(result.start, result.end);
    // Tell the parent the document changed — `bind:value` alone does not fire
    // the input event its autosave listens for.
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
  }

  export function handleShortcut(event) {
    if (!(event.metaKey || event.ctrlKey) || event.altKey) return false;
    const key = event.key.toLowerCase();
    // Deliberately not ⌘K: that is the global command palette, and taking it
    // over inside one textarea would be worse than lacking a link shortcut.
    const map = { b: 'bold', i: 'italic', e: 'code' };
    if (!map[key]) return false;
    event.preventDefault();
    run(map[key]);
    return true;
  }

  const GROUPS = [
    [
      { action: 'h1',    label: 'H1',     title: 'Heading 1' },
      { action: 'h2',    label: 'H2',     title: 'Heading 2' },
      { action: 'h3',    label: 'H3',     title: 'Heading 3' },
    ],
    [
      { action: 'bold',   icon: 'bold',   title: 'Bold (Ctrl/Cmd+B)' },
      { action: 'italic', icon: 'italic', title: 'Italic (Ctrl/Cmd+I)' },
      { action: 'strike', icon: 'strike', title: 'Strikethrough' },
      { action: 'code',   icon: 'code',   title: 'Inline code (Ctrl/Cmd+E)' },
    ],
    [
      { action: 'ul',    icon: 'ul',     title: 'Bulleted list' },
      { action: 'ol',    icon: 'ol',     title: 'Numbered list' },
      { action: 'task',  icon: 'task',   title: 'Task list' },
      { action: 'quote', icon: 'quote',  title: 'Quote' },
    ],
    [
      { action: 'link',      icon: 'link',      title: 'Link' },
      { action: 'codeblock', icon: 'codeblock', title: 'Code block' },
      { action: 'hr',        icon: 'hr',        title: 'Horizontal rule' },
    ],
  ];

  const btn =
    'flex-shrink-0 h-7 min-w-[1.75rem] px-1.5 flex items-center justify-center rounded-md ' +
    'text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-100 ' +
    'hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors duration-150 ' +
    'text-xs font-semibold';
</script>

<!-- Scrolls rather than wraps: a wrapping toolbar changes height and shoves the
     editor around as the window narrows. -->
<div
  class="flex items-center gap-0.5 px-2 py-1.5 flex-shrink-0 overflow-x-auto
         border-b border-gray-200 dark:border-gray-700
         bg-gray-50/60 dark:bg-gray-800/40"
  role="toolbar"
  aria-label="Formatting"
>
  {#each GROUPS as group, g}
    {#if g > 0}
      <div class="w-px h-4 bg-gray-200 dark:bg-gray-600 mx-1 flex-shrink-0"></div>
    {/if}
    {#each group as item}
      <button
        type="button"
        on:click={() => run(item.action)}
        title={item.title}
        aria-label={item.title}
        class={btn}
      >
        {#if item.label}
          {item.label}
        {:else}
          <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor"
               stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">
            {#if item.icon === 'bold'}
              <path d="M6 4h7a4 4 0 0 1 0 8H6zM6 12h8a4 4 0 0 1 0 8H6z" />
            {:else if item.icon === 'italic'}
              <path d="M19 4h-9M14 20H5M15 4 9 20" />
            {:else if item.icon === 'strike'}
              <path d="M4 12h16M17.5 7A4.5 4.5 0 0 0 13 4h-2a3.5 3.5 0 0 0-1.6 6.6M7 17a4.5 4.5 0 0 0 4 3h2a3.5 3.5 0 0 0 3.3-4.7" />
            {:else if item.icon === 'code'}
              <path d="m10 8-4 4 4 4M14 8l4 4-4 4" />
            {:else if item.icon === 'ul'}
              <path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01" />
            {:else if item.icon === 'ol'}
              <path d="M10 6h11M10 12h11M10 18h11M4 6h1v4M4 10h2M6 18H4c0-1 2-2 2-3s-1-1-2-1" />
            {:else if item.icon === 'task'}
              <path d="M11 6h10M11 12h10M11 18h10M3 6l1.5 1.5L7 5M3 17l1.5 1.5L7 16" />
            {:else if item.icon === 'quote'}
              <path d="M6 17h3l2-4V7H5v6h3zM15 17h3l2-4V7h-6v6h3z" />
            {:else if item.icon === 'link'}
              <path d="M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.7 1.7" />
              <path d="M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7l1.7-1.7" />
            {:else if item.icon === 'codeblock'}
              <rect x="3" y="4" width="18" height="16" rx="2" />
              <path d="m9 10-2 2 2 2M15 10l2 2-2 2" />
            {:else if item.icon === 'hr'}
              <path d="M3 12h18M6 7h12M6 17h12" opacity="0.9" />
            {/if}
          </svg>
        {/if}
      </button>
    {/each}
  {/each}

  {#if !compact}
    <div class="flex-1"></div>
    <span class="hidden sm:inline text-[10px] text-gray-400 dark:text-gray-500 flex-shrink-0 pr-1">
      Markdown
    </span>
  {/if}
</div>
