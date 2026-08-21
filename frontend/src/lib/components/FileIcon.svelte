<script>
  import { fileKind } from '$lib/fileTypes.js';

  export let node = null;
  /** sm (list rows) | lg (grid tiles) */
  export let size = 'sm';

  $: kind = fileKind(node);

  // One accent per kind, so a folder of mixed content is scannable at a glance.
  const ACCENT = {
    folder:  'text-blue-500 dark:text-blue-400',
    image:   'text-purple-500 dark:text-purple-400',
    video:   'text-pink-500 dark:text-pink-400',
    audio:   'text-amber-500 dark:text-amber-400',
    pdf:     'text-red-500 dark:text-red-400',
    doc:     'text-sky-600 dark:text-sky-400',
    sheet:   'text-green-600 dark:text-green-400',
    slides:  'text-orange-500 dark:text-orange-400',
    text:    'text-gray-500 dark:text-gray-400',
    code:    'text-teal-500 dark:text-teal-400',
    archive: 'text-yellow-600 dark:text-yellow-500',
    file:    'text-gray-400 dark:text-gray-500',
  };

  const SIZES = { sm: 'w-5 h-5', lg: 'w-12 h-12' };
  $: cls = `${SIZES[size] ?? SIZES.sm} ${ACCENT[kind] ?? ACCENT.file} flex-shrink-0`;
</script>

<svg class={cls} viewBox="0 0 24 24" fill="none" stroke="currentColor"
     stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
  {#if kind === 'folder'}
    <path d="M3 7a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
  {:else if kind === 'image'}
    <rect x="3" y="4" width="18" height="16" rx="2" />
    <circle cx="8.5" cy="9.5" r="1.5" /><path d="m21 16-5-5-6.5 7" />
  {:else if kind === 'video'}
    <rect x="2" y="5" width="14" height="14" rx="2" /><path d="m22 8-6 4 6 4z" />
  {:else if kind === 'audio'}
    <path d="M9 18V6l10-2v12" /><circle cx="6" cy="18" r="3" /><circle cx="16" cy="16" r="3" />
  {:else if kind === 'archive'}
    <path d="M4 6a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2z" />
    <path d="M12 4v3M12 9v2M12 13v2" />
  {:else if kind === 'code'}
    <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" /><path d="M14 3v5h5" />
    <path d="m10 12-2 2 2 2M14 12l2 2-2 2" />
  {:else if kind === 'sheet'}
    <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" /><path d="M14 3v5h5" />
    <path d="M8 13h8M8 17h8M12 13v4" />
  {:else}
    <!-- Generic document: pdf, doc, slides, text and anything unrecognised -->
    <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" /><path d="M14 3v5h5" />
    <path d="M9 13h6M9 17h4" />
  {/if}
</svg>
