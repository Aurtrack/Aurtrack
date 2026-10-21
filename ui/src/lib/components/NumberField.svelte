<script lang="ts">
  import { tick } from "svelte";

  import { formatNumber, t } from "../i18n.svelte";

  interface Props {
    value: number;
    min?: number;
    max?: number;
    step?: number;
    precision?: number;
    suffix?: string;
    disabled?: boolean;
    onchange: (value: number) => void;
  }

  let {
    value,
    min = -Infinity,
    max = Infinity,
    step = 1,
    precision,
    suffix = "",
    disabled = false,
    onchange,
  }: Props = $props();

  type Drag = {
    startX: number;
    startValue: number;
    moved: boolean;
  };

  // svelte-ignore state_referenced_locally
  let current = $state(value);
  let editing = $state(false);
  let inputValue = $state("");
  let inputElement = $state<HTMLInputElement | null>(null);
  let dragging = $state(false);
  let dirty = $state(false);
  let drag: Drag | null = null;
  let decimals = $derived(precision ?? (step < 1 ? 2 : 0));

  $effect(() => {
    if (editing || dragging) return;
    if (dirty) {
      if (Math.abs(value - current) < 1e-9) dirty = false;
      return;
    }
    current = value;
  });

  function clamp(next: number): number {
    return Math.min(max, Math.max(min, next));
  }

  function normalize(next: number): number {
    if (!Number.isFinite(next)) return current;
    let result = clamp(next);
    if (step > 0) result = Math.round(result / step) * step;
    return Number(result.toFixed(6));
  }

  function display(valueToShow: number): string {
    return `${formatNumber(valueToShow, decimals)}${suffix}`;
  }

  function commitValue(next: number) {
    current = next;
    dirty = true;
    onchange(next);
  }

  function startEdit() {
    if (disabled) return;
    editing = true;
    inputValue = current.toFixed(decimals);
    void tick().then(() => {
      inputElement?.focus();
      inputElement?.select();
    });
  }

  function commitEdit() {
    if (!editing) return;
    const parsed = Number(inputValue.replace(",", ".").trim());
    editing = false;
    if (Number.isFinite(parsed)) {
      commitValue(normalize(parsed));
    }
  }

  function cancelEdit() {
    editing = false;
    dirty = false;
    current = value;
  }

  function stepBy(delta: number) {
    if (disabled) return;
    commitValue(normalize(current + delta * step));
  }

  function handleMouseDown(event: MouseEvent) {
    if (disabled || event.button !== 0) return;
    event.preventDefault();
    drag = { startX: event.clientX, startValue: current, moved: false };
    dragging = false;

    const move = (moveEvent: MouseEvent) => {
      if (!drag) return;
      const delta = moveEvent.clientX - drag.startX;
      if (!drag.moved && Math.abs(delta) < 3) return;
      drag.moved = true;
      dragging = true;
      current = normalize(drag.startValue + (delta / 4) * step);
      moveEvent.preventDefault();
    };

    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      const wasDragging = drag?.moved ?? false;
      const next = current;
      drag = null;
      dragging = false;
      if (wasDragging) {
        commitValue(next);
      } else {
        startEdit();
      }
    };

    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "Enter") {
      commitEdit();
      event.preventDefault();
    } else if (event.key === "Escape") {
      cancelEdit();
      event.preventDefault();
    } else if (event.key === "ArrowUp" || event.key === "ArrowDown") {
      event.preventDefault();
      const direction = event.key === "ArrowUp" ? 1 : -1;
      const next = normalize(current + direction * step);
      inputValue = next.toFixed(decimals);
      commitValue(next);
    }
  }
</script>

<div
  class="number-field"
  class:disabled
  class:dragging
  role="group"
  aria-label={suffix || undefined}
>
  <button
    type="button"
    class="number-step"
    tabindex="-1"
    title={t("number.decrease")}
    disabled={disabled}
    onclick={() => stepBy(-1)}
  >
    ‹
  </button>
  {#if editing}
    <input
      class="number-input"
      bind:this={inputElement}
      bind:value={inputValue}
      inputmode="decimal"
      disabled={disabled}
      onblur={commitEdit}
      onkeydown={handleKeydown}
    />
    {#if suffix}<span class="number-suffix">{suffix}</span>{/if}
  {:else}
    <button
      type="button"
      class="number-value"
      title={t("number.dragHint")}
      disabled={disabled}
      onmousedown={handleMouseDown}
    >
      <span>{display(current)}</span>
    </button>
  {/if}
  <button
    type="button"
    class="number-step"
    tabindex="-1"
    title={t("number.increase")}
    disabled={disabled}
    onclick={() => stepBy(1)}
  >
    ›
  </button>
</div>
