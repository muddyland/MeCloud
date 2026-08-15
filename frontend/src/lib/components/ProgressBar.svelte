<script>
  import { busy } from '$lib/stores/activity.js';

  // A request that resolves in 80 ms should not produce a visible flash, and a
  // bar that vanishes the instant the last request lands reads as a glitch.
  // Show after a short delay, then keep it up for a minimum beat.
  const SHOW_DELAY = 180;
  const MIN_VISIBLE = 400;

  let visible = false;
  let showTimer;
  let hideTimer;
  let shownAt = 0;

  $: schedule($busy);

  function schedule(isBusy) {
    if (typeof window === 'undefined') return;
    if (isBusy) {
      clearTimeout(hideTimer);
      if (visible || showTimer) return;
      showTimer = setTimeout(() => {
        showTimer = undefined;
        visible = true;
        shownAt = Date.now();
      }, SHOW_DELAY);
    } else {
      clearTimeout(showTimer);
      showTimer = undefined;
      if (!visible) return;
      const remaining = Math.max(0, MIN_VISIBLE - (Date.now() - shownAt));
      hideTimer = setTimeout(() => { visible = false; }, remaining);
    }
  }
</script>

<!-- Fixed rather than in flow: the mail layout is exactly 100vh tall, and a bar
     that takes up space would push it into overflow every time it appeared. -->
<div class="fixed top-0 inset-x-0 h-0.5 z-[100] overflow-hidden pointer-events-none"
     aria-hidden="true">
  {#if visible}
    <div class="absolute inset-y-0 w-1/3 rounded-full bg-blue-500 dark:bg-blue-400 progress-sweep"></div>
  {/if}
</div>

<style>
  .progress-sweep {
    animation: sweep 1.1s cubic-bezier(0.65, 0, 0.35, 1) infinite;
  }

  @keyframes sweep {
    0%   { left: -35%; }
    100% { left: 100%; }
  }

  @media (prefers-reduced-motion: reduce) {
    .progress-sweep {
      animation: none;
      left: 0;
      width: 100%;
      opacity: 0.5;
    }
  }
</style>
