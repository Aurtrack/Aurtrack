<script lang="ts">
  import { onMount } from "svelte";

  import { errorMessage } from "../api";
  import { t } from "../i18n.svelte";
  import { session } from "../session.svelte";

  type Plugin = {
    id: string;
    name: string;
    vendor: string;
    version: string;
    path: string;
  };
  type Extension = {
    id: string;
    name: string;
    version: string;
    directory: string;
    commands: { id: string; name: string; executable: string }[];
  };

  let plugins = $state<Plugin[]>([]);
  let extensions = $state<Extension[]>([]);
  let pluginDirectory = $state("");
  let extensionDirectory = $state("");
  let failures = $state<string[]>([]);
  let loading = $state(true);
  let error = $state("");
  let result = $state("");

  async function refresh() {
    loading = true;
    error = "";
    failures = [];
    try {
      const [pluginReport, extensionReport] = await Promise.all([
        session.scanPlugins(),
        session.scanExtensions(),
      ]);
      plugins = pluginReport.plugins;
      pluginDirectory = pluginReport.directory;
      extensions = extensionReport.extensions;
      extensionDirectory = extensionReport.directory;
      failures = [
        ...pluginReport.failures.map(
          (failure) => `${failure.path}: ${failure.reason}`,
        ),
        ...extensionReport.failures.map(
          (failure) => `${failure.path}: ${failure.reason}`,
        ),
      ];
    } catch (cause) {
      error = errorMessage(cause);
    } finally {
      loading = false;
    }
  }

  async function runCommand(extension: Extension, command: string) {
    result = "";
    try {
      const data = await session.callExtension(extension.id, command, {});
      result = JSON.stringify(data, null, 2);
    } catch (cause) {
      result = errorMessage(cause);
    }
  }

  onMount(() => {
    void refresh();
  });
</script>

<div class="modal-backdrop">
  <section class="modal plugin-manager" role="dialog" aria-modal="true">
    <div class="modal-head">
      <span>{t("plugins.title")}</span>
      <button class="ghost small" onclick={() => session.pluginManagerOpen = false}>
        {t("plugins.close")}
      </button>
    </div>
    <div class="plugin-toolbar">
      <button class="ghost small" disabled={loading} onclick={() => void refresh()}>
        {t("plugins.refresh")}
      </button>
      {#if loading}
        <span class="plugin-note">{t("plugins.loading")}</span>
      {/if}
    </div>
    {#if error}
      <p class="modal-text error-text">{error}</p>
    {/if}

    <div class="plugin-columns">
      <section class="plugin-column">
        <header>
          <span>{t("plugins.clap")}</span>
          <code>{pluginDirectory}</code>
        </header>
        {#if plugins.length === 0}
          <p class="plugin-empty">{t("plugins.empty")}</p>
        {:else}
          <ul class="plugin-list">
            {#each plugins as plugin (plugin.id + plugin.path)}
              <li>
                <strong>{plugin.name || plugin.id}</strong>
                <span>{plugin.vendor} · {plugin.version}</span>
                <code>{plugin.id}</code>
              </li>
            {/each}
          </ul>
        {/if}
      </section>

      <section class="plugin-column">
        <header>
          <span>{t("plugins.extensions")}</span>
          <code>{extensionDirectory}</code>
        </header>
        {#if extensions.length === 0}
          <p class="plugin-empty">{t("plugins.empty")}</p>
        {:else}
          <ul class="plugin-list">
            {#each extensions as extension (extension.id)}
              <li>
                <strong>{extension.name}</strong>
                <span>{extension.version} · {extension.id}</span>
                <div class="plugin-commands">
                  {#each extension.commands as command (command.id)}
                    <button
                      class="ghost small"
                      onclick={() => void runCommand(extension, command.id)}
                    >
                      {command.name}
                    </button>
                  {/each}
                </div>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    </div>

    {#if failures.length > 0}
      <div class="plugin-failures">
        {#each failures as failure}
          <div>{failure}</div>
        {/each}
      </div>
    {/if}

    {#if result}
      <pre class="plugin-result">{result}</pre>
    {/if}
  </section>
</div>
