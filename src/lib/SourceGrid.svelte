<script lang="ts">
  // T064 — Grade de fontes ativas simultâneas (US6). Puramente apresentacional:
  // a lógica de iniciar/parar/expandir fica em +page.svelte.
  import SourceCard from "./SourceCard.svelte";
  import { MAX_CONCURRENT_SOURCES, type ActiveSource } from "./types";

  let {
    sources,
    expandedId,
    onSelect,
    onStop,
  }: {
    sources: ActiveSource[];
    expandedId: string | null;
    onSelect: (id: string) => void;
    onStop: (source: ActiveSource) => void;
  } = $props();
</script>

<div class="source-grid">
  {#each sources as source (source.id)}
    <SourceCard
      {source}
      expanded={source.id === expandedId}
      onExpand={() => onSelect(source.id)}
      onStop={() => onStop(source)}
    />
  {/each}

  {#if sources.length < MAX_CONCURRENT_SOURCES}
    <div class="ghost-card">
      <span class="plus">+</span>
      <span class="label">Adicionar fonte</span>
      <span class="hint">
        Selecione um dispositivo ou uma câmera RTSP na barra lateral
      </span>
    </div>
  {/if}
</div>

<style>
  .source-grid {
    display: flex;
    flex-wrap: wrap;
    gap: 1.25rem;
  }

  .ghost-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.4rem;
    width: 336px;
    height: 260px;
    border: 1.5px dashed var(--accent);
    border-radius: 16px;
    opacity: 0.6;
    text-align: center;
    padding: 1rem;
  }

  .plus {
    font-size: 1.8em;
    font-weight: 700;
    color: var(--accent);
    line-height: 1;
  }

  .label {
    font-weight: 600;
    font-size: 0.9em;
  }

  .hint {
    font-size: 0.75em;
    color: var(--muted);
  }
</style>
