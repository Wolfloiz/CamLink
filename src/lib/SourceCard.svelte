<script lang="ts">
  // T064 — Card compacto de uma fonte ativa na grade multi-fonte (US6).
  // Miniatura ao vivo (reaproveita Preview.svelte) + status + stats; clique
  // no card expande os controles completos em +page.svelte.
  import Preview from "./Preview.svelte";
  import { isSessionActive, isSessionError, type ActiveSource } from "./types";

  let {
    source,
    expanded,
    onExpand,
    onStop,
  }: {
    source: ActiveSource;
    expanded: boolean;
    onExpand: () => void;
    onStop: () => void;
  } = $props();

  let stopping = $state(false);

  async function handleStop(event: MouseEvent) {
    event.stopPropagation();
    stopping = true;
    try {
      await onStop();
    } finally {
      stopping = false;
    }
  }

  const statusLabel = $derived(
    isSessionError(source.state)
      ? "Erro"
      : source.state === "streaming"
        ? "Transmitindo"
        : source.state === "reconnecting"
          ? "Reconectando"
          : source.state === "source_lost"
            ? "Sinal perdido"
            : source.state === "starting"
              ? "Iniciando"
              : source.state,
  );
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div
  class="source-card"
  class:expanded
  onclick={onExpand}
  role="button"
  tabindex="0"
>
  <div class="thumb">
    <span
      class="status-pill"
      class:live={isSessionActive(source.state)}
      class:error={isSessionError(source.state)}
    >
      <span class="dot"></span>
      {statusLabel}
    </span>
    <span class="type-icon">{source.kind === "rtsp" ? "📡" : "📱"}</span>
    <Preview sessionId={source.sessionId} />
  </div>

  <div class="name-row">
    <span class="name">{source.name}</span>
    <span class="fps">{source.stats ? `${source.stats.fps.toFixed(1)} fps` : "—"}</span>
  </div>
  <span class="meta">{source.meta}</span>

  <div class="card-actions">
    <button
      type="button"
      class="stop-btn"
      disabled={stopping}
      onclick={handleStop}
    >
      {stopping ? "Parando..." : "Parar"}
    </button>
  </div>
</div>

<style>
  .source-card {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    width: 336px;
    padding: 1rem;
    background: var(--card-bg);
    border: 1px solid var(--card-border);
    border-radius: 16px;
    cursor: pointer;
    text-align: left;
    font: inherit;
    color: inherit;
  }

  .source-card.expanded {
    border-color: var(--accent);
  }

  .thumb {
    position: relative;
    border-radius: 12px;
    overflow: hidden;
  }

  .thumb :global(.preview) {
    aspect-ratio: 16 / 9;
  }

  .status-pill {
    position: absolute;
    top: 0.6rem;
    left: 0.6rem;
    z-index: 1;
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    font-size: 0.72em;
    font-weight: 600;
    padding: 0.2rem 0.6rem;
    border-radius: 999px;
    background: rgba(107, 114, 128, 0.35);
    color: #f5f5f7;
    backdrop-filter: blur(2px);
  }

  .status-pill .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: #9ca3af;
  }

  .status-pill.live .dot {
    background: #22c55e;
  }

  .status-pill.error .dot {
    background: #ef4444;
  }

  .type-icon {
    position: absolute;
    top: 0.6rem;
    right: 0.6rem;
    z-index: 1;
    font-size: 0.9em;
  }

  .name-row {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.5rem;
  }

  .name {
    font-weight: 600;
    font-size: 0.9em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .fps {
    font-size: 0.75em;
    color: var(--muted);
    flex-shrink: 0;
  }

  .meta {
    font-size: 0.75em;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .card-actions {
    display: flex;
  }

  .stop-btn {
    padding: 0.35rem 0.8rem;
    border-radius: 8px;
    border: 1px solid rgba(220, 38, 38, 0.4);
    background: transparent;
    color: #dc2626;
    cursor: pointer;
    font-size: 0.8em;
    font-weight: 600;
  }

  .stop-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
