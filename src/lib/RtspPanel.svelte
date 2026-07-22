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
    onError,
    onStarted,
  }: {
    onError: (e: unknown) => void;
    /** Sessão RTSP ativa (preview usa o session_id). */
    onStarted: (id: string, response: StartStreamResponse) => void;
  } = $props();

  let sources = $state<RtspSource[]>([]);
  let running = $state<Record<string, boolean>>({});
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
      running = { ...running, [source.id]: true };
      onStarted(source.id, response);
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
      running = { ...running, [source.id]: false };
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
      running = { ...running, [source.id]: false };
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
          <div class="info">
            <strong>{source.name}</strong>
            <span class="url">{source.url}</span>
            {#if source.secret_ref}
              <span class="lock" title="Credencial guardada no cofre do sistema"
                >🔒</span
              >
            {/if}
          </div>
          <div class="actions">
            {#if running[source.id]}
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
              disabled={busy || running[source.id]}
              title={running[source.id]
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
    grid-template-columns: 1fr 1.5fr 1fr auto;
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
    background: #4a8cff;
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
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.5rem 0.75rem;
    border: 1px solid rgba(120, 120, 120, 0.25);
    border-radius: 8px;
  }

  .info {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    min-width: 0;
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

  @media (max-width: 640px) {
    .add-form {
      grid-template-columns: 1fr;
    }
  }
</style>
