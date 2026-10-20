<script lang="ts">
  import { t } from "../i18n.svelte";
  import { session } from "../session.svelte";

  function handleChange(event: Event) {
    const select = event.currentTarget as HTMLSelectElement;
    const path = select.value;
    select.value = "";
    if (path) void session.openRecent(path);
  }
</script>

<select
  class="recent-select"
  value=""
  disabled={session.busy || session.recentProjects.length === 0}
  title={t("recent.label")}
  onchange={handleChange}
>
  <option value="" disabled>
    {session.recentProjects.length === 0
      ? t("recent.none")
      : t("recent.label")}
  </option>
  {#each session.recentProjects as project}
    <option value={project.path}>{project.name}</option>
  {/each}
</select>
