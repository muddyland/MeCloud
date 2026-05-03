<script>
  import '../app.css';
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { darkMode, appName } from '$lib/stores/mail.js';
  import { getMe, getAppConfig } from '$lib/api.js';

  onMount(async () => {
    darkMode.init($darkMode);

    const config = await getAppConfig();
    appName.set(config.appName);

    const isAuthRoute = $page.url.pathname.startsWith('/auth');
    if (!isAuthRoute) {
      const me = await getMe();
      if (!me.authenticated) {
        goto('/auth/login');
      }
    }
  });
</script>

<svelte:head>
  <title>{$appName}</title>
</svelte:head>

<slot />
