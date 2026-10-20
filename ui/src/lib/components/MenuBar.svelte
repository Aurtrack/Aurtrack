<script lang="ts">
  import {
    getLocale,
    setLocale,
    t,
    type MessageKey,
  } from "../i18n.svelte";
  import { layout } from "../layout.svelte";
  import { session } from "../session.svelte";

  type MenuId = "file" | "edit" | "render" | "view" | "help";

  const menus: { id: MenuId; label: MessageKey }[] = [
    { id: "file", label: "menu.file" },
    { id: "edit", label: "menu.edit" },
    { id: "render", label: "menu.render" },
    { id: "view", label: "menu.view" },
    { id: "help", label: "menu.help" },
  ];

  let openMenu = $state<MenuId | null>(null);
  let locale = $derived(getLocale());

  $effect(() => {
    if (!openMenu) return;
    const closeOnOutside = (event: PointerEvent) => {
      const target = event.target as Element | null;
      if (!target?.closest(".top-menu")) openMenu = null;
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") openMenu = null;
    };
    window.addEventListener("pointerdown", closeOnOutside);
    window.addEventListener("keydown", closeOnEscape);
    return () => {
      window.removeEventListener("pointerdown", closeOnOutside);
      window.removeEventListener("keydown", closeOnEscape);
    };
  });

  function toggle(id: MenuId) {
    openMenu = openMenu === id ? null : id;
  }

  function hover(id: MenuId) {
    if (openMenu) openMenu = id;
  }

  function close() {
    openMenu = null;
  }

  function run(action: () => void) {
    close();
    action();
  }

  function chooseLocale(next: "zh-CN" | "en") {
    setLocale(next);
    close();
  }
</script>

<nav class="top-menu" aria-label={t("menu.label")}>
  {#each menus as menu}
    <div class="menu-slot">
      <button
        class="menu-button"
        class:active={openMenu === menu.id}
        aria-haspopup="menu"
        aria-expanded={openMenu === menu.id}
        onclick={() => toggle(menu.id)}
        onmouseenter={() => hover(menu.id)}
      >
        {t(menu.label)}
      </button>

      {#if openMenu === menu.id}
        <div class="menu-popover" role="menu">
          {#if menu.id === "file"}
            <button
              class="menu-item"
              role="menuitem"
              disabled={session.busy}
              onclick={() => run(() => void session.openProject())}
            >
              {t("menu.open")}
            </button>
            <button
              class="menu-item"
              role="menuitem"
              disabled={session.busy}
              onclick={() => run(() => void session.saveProject())}
            >
              {t("menu.save")}
            </button>
            <button
              class="menu-item"
              role="menuitem"
              disabled={session.busy}
              onclick={() => run(() => void session.saveProjectAs())}
            >
              {t("menu.saveAs")}
            </button>
            <div class="menu-separator"></div>
            <div class="menu-label">{t("menu.recent")}</div>
            {#if session.recentProjects.length === 0}
              <div class="menu-empty">{t("menu.noRecent")}</div>
            {:else}
              {#each session.recentProjects as project}
                <button
                  class="menu-item menu-recent"
                  role="menuitem"
                  title={project.path}
                  disabled={session.busy}
                  onclick={() => {
                    close();
                    void session.openRecent(project.path);
                  }}
                >
                  {project.name}
                </button>
              {/each}
            {/if}
            <div class="menu-separator"></div>
            <button
              class="menu-item"
              role="menuitem"
              onclick={() => run(() => void session.quit())}
            >
              {t("menu.quit")}
            </button>
          {:else if menu.id === "edit"}
            <button
              class="menu-item"
              role="menuitem"
              disabled={session.busy}
              onclick={() => run(() => void session.undo())}
            >
              {t("menu.undo")}
            </button>
            <button
              class="menu-item"
              role="menuitem"
              disabled={session.busy}
              onclick={() => run(() => void session.redo())}
            >
              {t("menu.redo")}
            </button>
          {:else if menu.id === "render"}
            <button
              class="menu-item"
              role="menuitem"
              disabled={session.busy}
              onclick={() => run(() => void session.render())}
            >
              {t("menu.renderWav")}
            </button>
          {:else if menu.id === "view"}
            <button
              class="menu-item"
              role="menuitem"
              onclick={() => run(() => layout.reset())}
            >
              {t("menu.resetLayout")}
            </button>
            <div class="menu-separator"></div>
            <div class="menu-label">{t("menu.language")}</div>
            <button
              class="menu-item"
              role="menuitemradio"
              aria-checked={locale === "zh-CN"}
              onclick={() => chooseLocale("zh-CN")}
            >
              <span>{t("menu.chinese")}</span>
              {#if locale === "zh-CN"}
                <span class="menu-check">✓</span>
              {/if}
            </button>
            <button
              class="menu-item"
              role="menuitemradio"
              aria-checked={locale === "en"}
              onclick={() => chooseLocale("en")}
            >
              <span>{t("menu.english")}</span>
              {#if locale === "en"}
                <span class="menu-check">✓</span>
              {/if}
            </button>
          {:else}
            <div class="menu-label">{t("menu.about")}</div>
            <div class="menu-note">{t("menu.version")}</div>
            <div class="menu-note">{t("menu.license")}</div>
          {/if}
        </div>
      {/if}
    </div>
  {/each}
</nav>
