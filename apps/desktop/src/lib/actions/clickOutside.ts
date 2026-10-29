/**
 * Calls `onOutside` on any pointerdown outside `node` (used to close
 * dropdown menus / popovers on an outside click). Listens on `pointerdown`
 * rather than `click` so it fires before a subsequent click handler on
 * another trigger button, matching how the design's menus behave.
 */
export function clickOutside(node: HTMLElement, onOutside: () => void): { destroy(): void } {
  function handle(e: PointerEvent): void {
    if (e.target instanceof Node && !node.contains(e.target)) {
      onOutside();
    }
  }
  document.addEventListener("pointerdown", handle, true);
  return {
    destroy() {
      document.removeEventListener("pointerdown", handle, true);
    },
  };
}
