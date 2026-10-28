<script lang="ts">
  import { terminalThemeId, terminalThemes, uiTheme } from "../stores/theme";
</script>

<div class="row">
  <label class="inline">
    UI
    <select bind:value={$uiTheme}>
      <option value="dark">Dark</option>
      <option value="light">Light</option>
    </select>
  </label>
  <label class="inline">
    Terminal
    <select bind:value={$terminalThemeId}>
      {#each terminalThemes as t (t.id)}
        <option value={t.id}>{t.name}</option>
      {/each}
    </select>
  </label>
</div>

<style>
  .inline {
    flex-direction: row;
    align-items: center;
    gap: 6px;
  }
</style>
