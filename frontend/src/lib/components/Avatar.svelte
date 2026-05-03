<script>
  export let name = '';
  export let email = '';
  export let size = 'md'; // sm | md | lg

  const COLORS = [
    '#1E88E5', '#00897B', '#E53935', '#8E24AA',
    '#FB8C00', '#00ACC1', '#43A047', '#E91E63',
    '#F4511E', '#6D4C41', '#546E7A', '#3949AB',
    '#D81B60', '#039BE5', '#7CB342', '#C0CA33',
  ];

  function initials(n, e) {
    if (n?.trim()) {
      const parts = n.trim().split(/\s+/);
      return parts.length >= 2
        ? (parts[0][0] + parts[parts.length - 1][0]).toUpperCase()
        : parts[0].slice(0, 2).toUpperCase();
    }
    return e ? e.slice(0, 2).toUpperCase() : '?';
  }

  function color(str) {
    let h = 0;
    for (let i = 0; i < str.length; i++) h = (str.charCodeAt(i) + ((h << 5) - h)) | 0;
    return COLORS[Math.abs(h) % COLORS.length];
  }

  const sizes = { sm: 'w-7 h-7 text-xs', md: 'w-9 h-9 text-sm', lg: 'w-11 h-11 text-base' };

  $: label   = initials(name, email);
  $: bgColor = color(name || email || '?');
</script>

<div
  class="rounded-full flex items-center justify-center font-semibold text-white select-none flex-shrink-0 {sizes[size]}"
  style="background-color: {bgColor}"
  aria-hidden="true"
>
  {label}
</div>
