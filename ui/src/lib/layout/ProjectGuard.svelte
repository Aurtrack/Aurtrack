<script lang="ts">
  import { t } from "../i18n.svelte";
  import { session } from "../session.svelte";
</script>

{#if session.pendingOpenPath}
  <div class="modal-backdrop">
    <div
      class="modal"
      role="dialog"
      aria-modal="true"
      aria-labelledby="open-title"
    >
      <div class="modal-head">
        <span class="title" id="open-title">{t("open.title")}</span>
      </div>
      <p class="modal-text">{t("open.message")}</p>
      <div class="modal-actions">
        <button
          class="primary"
          disabled={session.busy}
          onclick={() => void session.confirmPendingOpen()}
        >
          {t("open.confirm")}
        </button>
        <button disabled={session.busy} onclick={() => session.cancelPendingOpen()}>
          {t("open.cancel")}
        </button>
      </div>
    </div>
  </div>
{/if}

{#if session.recoveryAvailable}
  <div class="modal-backdrop">
    <div
      class="modal"
      role="dialog"
      aria-modal="true"
      aria-labelledby="recovery-title"
    >
      <div class="modal-head">
        <span class="title" id="recovery-title">{t("recovery.title")}</span>
      </div>
      <p class="modal-text">{t("recovery.message")}</p>
      <div class="modal-actions">
        <button
          class="primary"
          disabled={session.busy}
          onclick={() => void session.restoreRecovery()}
        >
          {t("recovery.restore")}
        </button>
        <button
          disabled={session.busy}
          onclick={() => void session.discardRecovery()}
        >
          {t("recovery.discard")}
        </button>
      </div>
    </div>
  </div>
{/if}

{#if session.closeRequested}
  <div class="modal-backdrop">
    <div
      class="modal"
      role="dialog"
      aria-modal="true"
      aria-labelledby="close-title"
    >
      <div class="modal-head">
        <span class="title" id="close-title">{t("close.title")}</span>
      </div>
      <p class="modal-text">{t("close.message")}</p>
      <div class="modal-actions">
        <button
          class="primary"
          disabled={session.busy}
          onclick={() => void session.saveAndClose()}
        >
          {t("close.save")}
        </button>
        <button
          disabled={session.busy}
          onclick={() => void session.discardAndClose()}
        >
          {t("close.discard")}
        </button>
        <button disabled={session.busy} onclick={() => session.cancelClose()}>
          {t("close.cancel")}
        </button>
      </div>
    </div>
  </div>
{/if}
