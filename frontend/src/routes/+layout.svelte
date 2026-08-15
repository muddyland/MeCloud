<script>
  import '../app.css';
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { darkMode, appName, inboxUnread } from '$lib/stores/mail.js';
  import { getMe, getAppConfig } from '$lib/api.js';
  import ProgressBar from '$lib/components/ProgressBar.svelte';
  import Spinner from '$lib/components/Spinner.svelte';

  let checkingAuth = true;

  // Badge the tab the way every other mail client does, so an unread count is
  // visible without switching to the app.
  $: title = $inboxUnread > 0 ? `(${$inboxUnread}) ${$appName}` : $appName;

  onMount(async () => {
    darkMode.init($darkMode);

    const isAuthRoute = $page.url.pathname.startsWith('/auth');

    try {
      const [config, me] = await Promise.all([
        getAppConfig(),
        isAuthRoute ? Promise.resolve({ authenticated: true }) : getMe(),
      ]);

      if (config?.appName) appName.set(config.appName);

      if (!isAuthRoute && !me.authenticated) {
        goto('/auth/login');
        return;
      }
    } catch (e) {
      // Never leave the user staring at a spinner because a probe failed —
      // render the app and let the individual views report their own errors.
      console.error('Startup check failed:', e);
    } finally {
      checkingAuth = false;
    }
  });
</script>

<svelte:head>
  <title>{title}</title>
</svelte:head>

<!-- Thin activity bar: any in-flight request drives it, so the app always shows
     it is doing something even when the work has no local spinner. -->
<ProgressBar />

{#if checkingAuth}
  <div class="fixed inset-0 flex flex-col items-center justify-center gap-3
              bg-gray-100 dark:bg-gray-950">
    <Spinner size="lg" label="Signing in" />
    <p class="text-sm text-gray-500 dark:text-gray-400">Opening your mailbox…</p>
  </div>
{:else}
  <slot />
{/if}
