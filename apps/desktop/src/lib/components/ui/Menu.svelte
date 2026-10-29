<!--
  Generic positioned dropdown / context menu (design: `.menu`). The caller
  supplies the anchor point (`x`,`y`, from a button's getBoundingClientRect
  or a right-click's clientX/clientY) and menu items as a slot, so one
  component covers both "dropdown below a button" and "right-click context
  menu" per the design. Closes on Escape or an outside click; stays inside
  the window (clamped) so a context menu opened near an edge is never cut off.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { clickOutside } from "../../actions/clickOutside";

  /** Anchor X. With `align="right"`, this is the menu's right edge. */
  export let x: number;
  export let y: number;
  export let align: "left" | "right" = "left";
  export let onClose: () => void;

  let el: HTMLDivElement;
  let left = x;
  let top = y;

  onMount(() => {
    const rect = el.getBoundingClientRect();
    let l = align === "right" ? x - rect.width : x;
    let t = y;
    const margin = 8;
    if (l + rect.width > window.innerWidth - margin) l = window.innerWidth - margin - rect.width;
    if (l < margin) l = margin;
    if (t + rect.height > window.innerHeight - margin) t = y - rect.height;
    if (t < margin) t = margin;
    left = l;
    top = t;
    el.focus();
  });

  function keydown(e: KeyboardEvent): void {
    if (e.key === "Escape") {
      e.stopPropagation();
      onClose();
    }
  }
</script>

<svelte:window on:keydown={keydown} />

<div
  class="menu"
  role="menu"
  tabindex="-1"
  bind:this={el}
  style="left:{left}px;top:{top}px"
  use:clickOutside={onClose}
>
  <slot />
</div>
