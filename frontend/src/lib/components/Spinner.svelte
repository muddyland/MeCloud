<script>
  /** xs → inline in a button label; sm/md → inline states; lg → full-pane. */
  export let size = 'sm';
  /** Accessible label; set to '' when a sibling already announces the state. */
  export let label = 'Loading';
  /** Tailwind colour classes for the moving arc. */
  export let accent = 'border-t-blue-500';
  /** Extra classes on the wrapper. */
  export let cls = '';

  const sizes = {
    xs: 'w-3 h-3 border-[1.5px]',
    sm: 'w-4 h-4 border-2',
    md: 'w-6 h-6 border-2',
    lg: 'w-8 h-8 border-[3px]',
  };
</script>

<span
  class="inline-block flex-shrink-0 rounded-full animate-spin
         border-gray-300/70 dark:border-gray-600/70 {accent} {sizes[size] ?? sizes.sm} {cls}"
  role="status"
  aria-label={label || undefined}
  aria-hidden={label ? undefined : 'true'}
></span>
{#if label}
  <span class="sr-only">{label}</span>
{/if}
