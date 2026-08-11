<script lang="ts">
  // T053 — Painel de fontes IP/RTSP (US4/FR-018): nome/URL/senha (senha vai
  // pro cofre do SO, nunca pra config), start/stop por fonte, remover limpa
  // o segredo. Cada fonte ativa vira uma webcam virtual independente.
  import {
    addRtspSource,
    listRtspSources,
    removeRtspSource,
    startRtsp,
    stopRtsp,
  } from "./api";
  import type { RtspSource, StartStreamResponse } from "./types";

  let {
    activeIds,
    onError,
    onStarted,
    onStopped,
  }: {
    /** IDs das fontes RTSP ativas AGORA (fonte única de verdade: `sources`
     * em +page.svelte) — evita este painel ter seu próprio estado de
     * "rodando" que poderia dessincronizar do card na grade. */
    activeIds: string[];
    onError: (e: unknown) => void;
    /** Fonte iniciada (grade em +page.svelte adiciona um card). */
    onStarted: (source: RtspSource, response: StartStreamResponse) => void;
    /** Fonte parada por aqui (grade em +page.svelte remove o card). */
    onStopped: (id: string) => void;
  } = $props();

  let sources = $state<RtspSource[]>([]);
  let busy = $state(false);

  let name = $state("");
  let url = $state("");
  let password = $state("");

  async function refresh() {
    try {
      sources = await listRtspSources();
    } catch (e) {
      onError(e);
    }
  }

  $effect(() => {
    refresh();
  });

  async function handleAdd(event: SubmitEvent) {
    event.preventDefault();
    if (!name.trim() || !url.trim()) return;
    busy = true;
    try {
      await addRtspSource(name.trim(), url.trim(), password ? password : null);
      name = "";
      url = "";
      password = "";
      await refresh();
    } catch (e) {
      onError(e);
    } finally {
      busy = false;
    }
  }

  async function handleStart(source: RtspSource) {
    busy = true;
    try {
      const response = await startRtsp(source.id);
      onStarted(source, response);
    } catch (e) {
      onError(e);
    } finally {
      busy = false;
    }
  }

  async function handleStop(source: RtspSource) {
    busy = true;
    try {
      await stopRtsp(source.id);
      onStopped(source.id);
    } catch (e) {
      onError(e);
    } finally {
      busy = false;
    }
  }

  async function handleRemove(source: RtspSource) {
    busy = true;
    try {
      await removeRtspSource(source.id);
      await refresh();
    } catch (e) {
      onError(e);
    } finally {
      busy = false;
    }
  }
</script>

<div class="rtsp-panel">
  <form class="add-form" onsubmit={handleAdd}>
    <input placeholder="Nome (ex.: Câmera do portão)" bind:value={name} />
    <input placeholder="rtsp://192.168.0.42:554/stream1" bind:value={url} />
    <input
      type="password"
      placeholder="Senha (opcional — vai pro cofre do sistema)"
      bind:value={password}
      autocomplete="off"
    />
    <button type="submit" disabled={busy || !name.trim() || !url.trim()}>
      Adicionar
    </button>
  </form>

  {#if sources.length === 0}
    <p class="empty">Nenhuma fonte RTSP cadastrada.</p>
  {:else}
    <ul class="sources">
      {#each sources as source (source.id)}
        <li>
          <div class="row-top">
            <div class="info">
              <strong>{source.name}</strong>
              {#if source.secret_ref}
                <span class="lock" title="Credencial guardada no cofre do sistema"
                  >🔒</span
                >
              {/if}
            </div>
            <span class="badge" class:active={activeIds.includes(source.id)}>
              <span class="dot"></span>
              {activeIds.includes(source.id) ? "Ativa" : "Parada"}
            </span>
          </div>
          <span class="url">{source.url}</span>
          <div class="actions">
            {#if activeIds.includes(source.id)}
              <button
                type="button"
                disabled={busy}
                onclick={() => handleStop(source)}
              >
                Parar
              </button>
            {:else}
              <button
                type="button"
                disabled={busy}
                onclick={() => handleStart(source)}
              >
                Iniciar
              </button>
            {/if}
            <button
              type="button"
              class="danger"
              disabled={busy || activeIds.includes(source.id)}
              title={activeIds.includes(source.id)
                ? "Pare a fonte antes de remover"
                : "Remove a fonte e o segredo do cofre"}
              onclick={() => handleRemove(source)}
            >
              Remover
            </button>
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .rtsp-panel {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .add-form {
    display: grid;
    grid-template-columns: 1fr;
    gap: 0.5rem;
  }

  .add-form input {
    padding: 0.4rem 0.6rem;
    border-radius: 6px;
    border: 1px solid rgba(120, 120, 120, 0.4);
    background: transparent;
    color: inherit;
    font-size: 0.85em;
  }

  .add-form button {
    padding: 0.4rem 0.9rem;
    border-radius: 6px;
    border: none;
    background: #6d5cf5;
    color: white;
    cursor: pointer;
    font-weight: 600;
    font-size: 0.85em;
  }

  .add-form button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .empty {
    font-size: 0.85em;
    opacity: 0.6;
    margin: 0;
  }

  .sources {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .sources li {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    padding: 0.6rem 0.75rem;
    border: 1px solid rgba(120, 120, 120, 0.25);
    border-radius: 10px;
  }

  .row-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }

  .info {
    display: flex;
    align-items: baseline;
    gap: 0.4rem;
    min-width: 0;
  }

  .badge {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    font-size: 0.72em;
    font-weight: 600;
    padding: 0.15rem 0.5rem;
    border-radius: 999px;
    flex-shrink: 0;
    background: rgba(107, 114, 128, 0.15);
    color: #6b7280;
  }

  .badge .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: currentColor;
  }

  .badge.active {
    background: rgba(34, 197, 94, 0.15);
    color: #15803d;
  }

  .url {
    font-size: 0.8em;
    opacity: 0.6;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .actions {
    display: flex;
    gap: 0.4rem;
    flex-shrink: 0;
  }

  .actions button {
    padding: 0.3rem 0.7rem;
    border-radius: 6px;
    border: 1px solid rgba(120, 120, 120, 0.4);
    background: transparent;
    color: inherit;
    cursor: pointer;
    font-size: 0.8em;
  }

  .actions button.danger {
    color: #b91c1c;
    border-color: rgba(220, 38, 38, 0.4);
  }

  .actions button:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
</style>
