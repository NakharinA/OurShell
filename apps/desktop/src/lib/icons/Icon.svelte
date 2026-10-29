<!--
  Thin wrapper around `<svg><use href="#i-name"/></svg>` so screens don't
  repeat that markup. `name` must match a `<symbol id="i-name">` defined in
  IconSprite.svelte (mounted once near the app root).
-->
<script lang="ts">
  export let name: string;
  export let size: "sm" | "md" | "lg" | undefined = undefined;
  let klass = "";
  export { klass as class };
</script>

<svg class="ic {size ? `ic-${size}` : ''} {klass}" aria-hidden="true" focusable="false">
  <use href={`#i-${name}`}></use>
</svg>
