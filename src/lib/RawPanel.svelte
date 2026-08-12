<script lang="ts">
  // T060 — Captura RAW (US5/FR-019/020): Snapshot (1 frame DNG na resolução
  // nativa do sensor) e Sequência (1-3 fps, cadência ajustada dinamicamente
  // pelo fork — o vídeo principal tem prioridade de banda). Oculto sem
  // caps.raw (FR-016/019): nunca falha silenciosa, mas também não faz
  // sentido oferecer um controle que o aparelho não suporta.
  import {
    getCapabilities,
    onRawProgress,
    rawSequenceStart,
    rawSequenceStop,
    rawSnapshot,
    setRawOutputDir,
  } from "./api";
  import {
    isRawJobFailed,
    isRawJobRunning,
    type DeviceCapabilities,
    type RawCaptureJob,
  } from "./types";

  let {
    sessionId,
    serial,
    onError,
  }: {
    sessionId: string;
    serial: string;
    onError: (e: unknown) => void;
  } = $props();

  let caps = $state<DeviceCapabilities | null>(null);
  let busy = $state(false);
  let sequenceRunning = $state(false);
  let requestedFps = $state(3);
  let grantedFps = $state<number | null>(null);
  let job = $state<RawCaptureJob | null>(null);
  let lastSnapshotPath = $state<string | null>(null);
  let outputDir = $state("");
  let unlistenProgress: (() => void) | null = null;

  // Sessão já carregada — `let` cru de propósito (ver ModeSelector).
  let capsLoadedFor: string | null = null;

  $effect(() => {
    // Não zera `caps` antes de recarregar: `get_capabilities` é um round-trip
    // até o aparelho e apagar o painel durante a espera era o "piscar" dos
    // controles. O guard evita refazer a chamada à toa.
    if (capsLoadedFor === sessionId) return;
    capsLoadedFor = sessionId;
    getCapabilities(serial)
      .then((c) => (caps = c))
      .catch(() => {
        // Sem capabilities, o painel simplesmente não aparece (ver markup) —
        // outros painéis (CameraControls) já reportam o erro de conexão.
      });
  });

  $effect(() => {
    const currentSession = sessionId;
    let cancelled = false;
    onRawProgress((event) => {
      if (cancelled || event.session_id !== currentSession) return;
      job = event.job;
      if (isRawJobRunning(event.job.state)) {
        sequenceRunning = true;
      } else {
        sequenceRunning = false;
      }
      if (isRawJobFailed(event.job.state)) {
        onError({ msg: event.job.state.failed });
      }
    }).then((fn) => {
      if (cancelled) {
        fn();
      } else {
        unlistenProgress = fn;
      }
    });
    return () => {
      cancelled = true;
      unlistenProgress?.();
      unlistenProgress = null;
    };
  });

  async function handleSnapshot() {
    busy = true;
    lastSnapshotPath = null;
    try {
      lastSnapshotPath = await rawSnapshot(sessionId);
    } catch (e) {
      onError(e);
    } finally {
      busy = false;
    }
  }

  async function handleSequenceStart() {
    busy = true;
    try {
      grantedFps = await rawSequenceStart(sessionId, requestedFps);
      sequenceRunning = true;
    } catch (e) {
      onError(e);
    } finally {
      busy = false;
    }
  }

  async function handleSequenceStop() {
    busy = true;
    try {
      await rawSequenceStop(sessionId);
      sequenceRunning = false;
    } catch (e) {
      onError(e);
    } finally {
      busy = false;
    }
  }

  async function handleOutputDirChange() {
    if (!outputDir.trim()) return;
    try {
      await setRawOutputDir(sessionId, outputDir.trim());
    } catch (e) {
      onError(e);
    }
  }

  function formatBytes(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    const mb = bytes / (1024 * 1024);
    return `${mb.toFixed(1)} MB`;
  }
</script>

{#if caps?.raw}
  <div class="raw-panel">
    <p class="hint">
      Sensor nativo: {caps.raw.sensor_size[0]}×{caps.raw.sensor_size[1]}
    </p>

    <label class="output-dir">
      Pasta de saída
      <input
        type="text"
        placeholder="Padrão: Pasta de Imagens/CamLink"
        bind:value={outputDir}
        onblur={handleOutputDirChange}
      />
    </label>

    <div class="actions">
      <button
        type="button"
        disabled={busy || sequenceRunning}
        onclick={handleSnapshot}
      >
        Snapshot RAW
      </button>

      {#if !sequenceRunning}
        <label class="fps-input">
          fps
          <input
            type="number"
            min="1"
            max="3"
            step="1"
            bind:value={requestedFps}
            disabled={busy}
          />
        </label>
        <button
          type="button"
          disabled={busy}
          onclick={handleSequenceStart}
        >
          Iniciar sequência
        </button>
      {:else}
        <button type="button" disabled={busy} onclick={handleSequenceStop}>
          Parar sequência
        </button>
      {/if}
    </div>

    {#if grantedFps !== null && sequenceRunning}
      <p class="hint">
        fps concedida: {grantedFps.toFixed(1)}
        {#if grantedFps < requestedFps}
          (reduzida pela banda disponível — vídeo principal tem prioridade)
        {/if}
      </p>
    {/if}

    {#if job && isRawJobRunning(job.state)}
      <p class="progress">
        {job.state.running.frames} frames · {formatBytes(
          job.state.running.bytes,
        )} · {job.state.running.effective_fps.toFixed(1)} fps
      </p>
    {/if}

    {#if lastSnapshotPath}
      <p class="hint">Salvo em {lastSnapshotPath}</p>
    {/if}
  </div>
{/if}

<style>
  .raw-panel {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }

  .output-dir {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    font-size: 0.85em;
  }

  .output-dir input {
    padding: 0.4rem 0.6rem;
    border-radius: 6px;
    border: 1px solid rgba(120, 120, 120, 0.4);
    background: transparent;
    color: inherit;
    font-size: 0.9em;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .actions button {
    padding: 0.35rem 0.8rem;
    border-radius: 6px;
    border: 1px solid rgba(120, 120, 120, 0.4);
    background: transparent;
    color: inherit;
    cursor: pointer;
    font-size: 0.85em;
  }

  .actions button:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .fps-input {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    font-size: 0.85em;
  }

  .fps-input input {
    width: 3.5rem;
    padding: 0.3rem;
    border-radius: 6px;
    border: 1px solid rgba(120, 120, 120, 0.4);
    background: transparent;
    color: inherit;
  }

  .hint,
  .progress {
    font-size: 0.8em;
    opacity: 0.75;
    margin: 0;
  }
</style>
