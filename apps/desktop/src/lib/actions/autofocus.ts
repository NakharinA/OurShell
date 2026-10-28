/**
 * Focuses the element as soon as it's mounted.
 *
 * Using the plain `autofocus` HTML attribute trips Svelte's `a11y_autofocus`
 * warning, because stealing focus on page load can disorient screen-reader
 * and keyboard users who land elsewhere. Here the only elements that use it
 * are the master-password fields on the unlock/create-vault screens, which
 * are each the sole input on a freshly mounted, full-screen view — moving
 * focus there is the expected behaviour and we want to keep it. Doing the
 * focus programmatically (via a Svelte action) rather than the static
 * attribute is the recommended way to get identical behaviour without the
 * lint warning.
 */
export function autofocus(node: HTMLElement): void {
  node.focus();
}
