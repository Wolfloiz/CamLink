<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import CameraControls from "$lib/CameraControls.svelte";
  import DeviceList from "$lib/DeviceList.svelte";
  import ModeSelector from "$lib/ModeSelector.svelte";
  import Preview from "$lib/Preview.svelte";
  import RawPanel from "$lib/RawPanel.svelte";
  import RtspPanel from "$lib/RtspPanel.svelte";
  import SourceGrid from "$lib/SourceGrid.svelte";
  import {
    listDeviceNicknames,
    onControlState,
    onSessionReplaced,
    onSessionState,
    setControl,
    setDeviceNickname,
    startStream,
    stopRtsp,
    stopStream,
  } from "$lib/api";
  import {
    DEFAULT_STREAM_CONFIG,
    isSessionError,
    MAX_CONCURRENT_SOURCES,
    type ActiveSource,
    type AndroidDevice,
    type RtspSource,
    type StartStreamResponse,
    type StreamConfig,
    type VideoCodec,
  } from "$lib/types";

  const RESOLUTIONS: Array<[number, number]> = [
    [1920, 1080],
    [1280, 720],
    [640, 480],
  ];
  const FPS_OPTIONS = [15, 24, 30, 60];

  let selectedDevice = $state<AndroidDevice | null>(null);
  let resolutionIndex = $state(0);
  let fps = $state(DEFAULT_STREAM_CONFIG.fps);
  let bitrateMbps = $state(DEFAULT_STREAM_CONFIG.bitrate / 1_000_000);
  let codec = $state<VideoCodec>(DEFAULT_STREAM_CONFIG.codec);
  const cameraId = "0";

  // US6 — Fontes ativas simultâneas (até MAX_CONCURRENT_SOURCES, T062): cada
  // uma vira um card na grade; no máximo uma fica "expandida" por vez
  // (controles completos), igual ao design aprovado no Penpot.
  let sources = $state<ActiveSource[]>([]);
  let expandedId = $state<string | null>(null);
  // T065e: apelidos persistidos por serial ("Câmera lateral" em vez do
  // model bruto do adb) — carregado uma vez; atualizado localmente junto
  // com a chamada que persiste, sem precisar recarregar tudo.
  let deviceNicknames = $state<Record<string, string>>({});
  onMount(() => {
    listDeviceNicknames()
      .then((n) => (deviceNicknames = n))
      .catch(() => {});
  });

  async function handleRenameDevice(serial: string, nickname: string) {
    const trimmed = nickname.trim();
    try {
      await setDeviceNickname(serial, trimmed);
    } catch (e) {
      applyError(e);
      return;
    }
    deviceNicknames = { ...deviceNicknames, [serial]: trimmed };
    if (!trimmed) delete deviceNicknames[serial];
    sources = sources.map((s) =>
      s.kind === "android" && s.serial === serial
        ? { ...s, name: trimmed || s.name }
        : s,
    );
  }
  const expandedSource = $derived(
    sources.find((s) => s.id === expandedId) ?? null,
  );

  let starting = $state(false);
  let errorMsg = $state<string | null>(null);
  let actionHint = $state<string | null>(null);

  const rtspActiveIds = $derived(
    sources.filter((s) => s.kind === "rtsp").map((s) => s.id),
  );

  // Aba da fonte selecionada na barra lateral (puramente visual — Android e
  // RTSP continuam podendo transmitir ao mesmo tempo, US6).
  let sourceTab = $state<"android" | "rtsp">("android");

  let unlistenSessionState: (() => void) | null = null;
  let unlistenSessionReplaced: (() => void) | null = null;
  let unlistenControlState: (() => void) | null = null;

  $effect(() => {
    // Só sessões Android emitem esses eventos (RTSP não tem, ainda) —
    // procuramos a fonte correspondente pelo session_id corrente.
    onSessionState((event) => {
      const idx = sources.findIndex((s) => s.sessionId === event.session_id);
      if (idx === -1) return;
      if (event.state === "idle") {
        const removedId = sources[idx].id;
        sources = sources.filter((_, i) => i !== idx);
        if (expandedId === removedId) expandedId = null;
        return;
      }
      // `session_state` chega a cada ~250ms POR sessão (research: mantém o
      // fps do stream atualizado) — com 4 fontes simultâneas (US6/T065) isso
      // é ~16 eventos/s reconstruindo o array inteiro de `sources`, mesmo
      // quando nada relevante mudou (fps oscila por ruído de arredondamento
      // no backend). Comparar antes de recriar o objeto evita acordar toda
      // a reatividade (grade + preview expandido) sem necessidade — achado
      // em bancada testando 3 celulares + 1 RTSP simultâneos (2026-08-08),
      // onde o preview "piscava" sob essa carga.
      // `fps` é uma EMA (float) que muda por ruído a cada tick — comparar
      // com o valor arredondado exibido na UI (`toFixed(1)`) em vez do float
      // cru, senão a comparação nunca bate e o guard vira um no-op.
      const current = sources[idx];
      if (
        current.state === event.state &&
        current.stats?.fps.toFixed(1) === event.stats.fps.toFixed(1) &&
        current.stats?.uptime_secs === event.stats.uptime_secs &&
        current.stats?.reconnects === event.stats.reconnects
      ) {
        return;
      }
      sources = sources.map((s, i) =>
        i === idx ? { ...s, state: event.state, stats: event.stats } : s,
      );
    }).then((fn) => {
      unlistenSessionState = fn;
    });
    // Rotação 90°/270° reinicia a sessão por baixo (T079) — realinha o
    // session_id da fonte sem o usuário perceber.
    onSessionReplaced((event) => {
      const idx = sources.findIndex((s) => s.sessionId === event.old_session_id);
      if (idx === -1) return;
      // Só o `sessionId` muda — `id` é a identidade estável da fonte, e
      // mexer nele aqui destruía/recriava o card inteiro (D2).
      sources = sources.map((s, i) =>
        i === idx
          ? {
              ...s,
              sessionId: event.response.session_id,
              state: event.response.state,
              stats: event.response.stats,
            }
          : s,
      );
    }).then((fn) => {
      unlistenSessionReplaced = fn;
    });
    // US3: modo inteligente corrente — CameraControls usa pra liberar ISO
    // manual só no modo pro; ModeSelector usa pra marcar o modo ativo.
    onControlState((event) => {
      const idx = sources.findIndex((s) => s.sessionId === event.session_id);
      if (idx === -1) return;
      sources = sources.map((s, i) =>
        i === idx ? { ...s, controlState: event.control_state } : s,
      );
    }).then((fn) => {
      unlistenControlState = fn;
    });
    return () => {
      unlistenSessionState?.();
      unlistenSessionState = null;
      unlistenSessionReplaced?.();
      unlistenSessionReplaced = null;
      unlistenControlState?.();
      unlistenControlState = null;
    };
  });

  onDestroy(() => {
    unlistenSessionState?.();
    unlistenSessionReplaced?.();
    unlistenControlState?.();
  });

  /**
   * switch_camera / girar 90°-270° reiniciam a sessão → session_id novo. A
   * identidade da fonte (`id`) NÃO muda: é a mesma câmera do mesmo aparelho,
   * e trocá-la faria a grade destruir e recriar o card (D2).
   */
  function adoptSession(sourceId: string, response: StartStreamResponse) {
    sources = sources.map((s) =>
      s.id === sourceId
        ? {
            ...s,
            sessionId: response.session_id,
            state: response.state,
            stats: response.stats,
            controlState: null, // sessão nova: modo corrente ainda não é conhecido
          }
        : s,
    );
  }

  async function handleTapToFocus(x: number, y: number) {
    if (!expandedSource || expandedSource.kind !== "android") return;
    try {
      await setControl(expandedSource.sessionId, { focus: { tap: { x, y } } });
    } catch (e) {
      applyError(e);
    }
  }

  function buildConfig(): StreamConfig {
    return {
      resolution: RESOLUTIONS[resolutionIndex],
      fps,
      bitrate: Math.round(bitrateMbps * 1_000_000),
      codec,
      camera_id: cameraId,
    };
  }

  function applyError(e: unknown) {
    if (e && typeof e === "object" && "msg" in e) {
      const appError = e as { msg: string; action_hint?: string | null };
      errorMsg = appError.msg;
      actionHint = appError.action_hint ?? null;
    } else {
      errorMsg = String(e);
    }
  }

  async function handleStart() {
    if (!selectedDevice) return;
    if (sources.length >= MAX_CONCURRENT_SOURCES) {
      applyError({ msg: `Limite de ${MAX_CONCURRENT_SOURCES} fontes simultâneas atingido.` });
      return;
    }
    if (sources.some((s) => s.kind === "android" && s.serial === selectedDevice!.serial)) {
      applyError({ msg: "Este dispositivo já está transmitindo." });
      return;
    }
    errorMsg = null;
    actionHint = null;
    starting = true;
    try {
      const response = await startStream(selectedDevice.serial, buildConfig());
      const newSource: ActiveSource = {
        kind: "android",
        // Identidade estável (ver `ActiveSource.id`): o serial não muda entre
        // restarts internos, o `session_id` muda.
        id: `android:${selectedDevice.serial}`,
        sessionId: response.session_id,
        name: deviceNicknames[selectedDevice.serial] || selectedDevice.model,
        meta: `adb · USB · ${selectedDevice.serial}`,
        state: response.state,
        stats: response.stats,
        serial: selectedDevice.serial,
        controlState: null,
      };
      sources = [...sources, newSource];
      expandedId = newSource.id;
    } catch (e) {
      applyError(e);
    } finally {
      starting = false;
    }
  }

  async function handleStopSource(source: ActiveSource) {
    try {
      if (source.kind === "android") {
        await stopStream(source.sessionId);
      } else {
        await stopRtsp(source.id);
      }
    } catch (e) {
      applyError(e);
      return;
    }
    sources = sources.filter((s) => s.id !== source.id);
    if (expandedId === source.id) expandedId = null;
  }

  function handleRtspStarted(source: RtspSource, response: StartStreamResponse) {
    const newSource: ActiveSource = {
      kind: "rtsp",
      id: source.id,
      sessionId: response.session_id,
      name: source.name,
      meta: source.url,
      state: response.state,
      stats: response.stats,
    };
    sources = [...sources, newSource];
    expandedId = newSource.id;
  }

  function handleRtspStopped(id: string) {
    sources = sources.filter((s) => s.id !== id);
    if (expandedId === id) expandedId = null;
  }
</script>

<div class="app">
  <header class="topbar">
    <div class="brand">
      <span class="brand-dot"></span>
      <span class="brand-name">CamLink</span>
    </div>
    <span class="source-count">{sources.length}/{MAX_CONCURRENT_SOURCES} fontes ativas</span>
  </header>

  <main class="layout">
    <aside class="sidebar">
      <div class="tabs">
        <button
          type="button"
          class:active={sourceTab === "android"}
          onclick={() => (sourceTab = "android")}
        >
          Dispositivo Android
        </button>
        <button
          type="button"
          class:active={sourceTab === "rtsp"}
          onclick={() => (sourceTab = "rtsp")}
        >
          Fontes RTSP
        </button>
      </div>

      {#if sourceTab === "android"}
        <div class="card">
          <DeviceList
            selectedSerial={selectedDevice?.serial ?? null}
            onSelect={(device) => (selectedDevice = device)}
            nicknames={deviceNicknames}
            onRename={handleRenameDevice}
          />
        </div>

        <div class="card">
          <h2>Configuração da sessão</h2>
          <div class="config-grid">
            <label>
              Resolução
              <select bind:value={resolutionIndex} disabled={starting}>
                {#each RESOLUTIONS as res, i (i)}
                  <option value={i}>{res[0]}x{res[1]}</option>
                {/each}
              </select>
            </label>

            <label>
              FPS alvo
              <select bind:value={fps} disabled={starting}>
                {#each FPS_OPTIONS as f (f)}
                  <option value={f}>{f}</option>
                {/each}
              </select>
            </label>

            <label>
              Bitrate
              <select bind:value={bitrateMbps} disabled={starting}>
                {#each [2, 4, 6, 8, 12, 20, 30, 50] as mb (mb)}
                  <option value={mb}>{mb} Mbps</option>
                {/each}
              </select>
            </label>

            <label>
              Codec
              <select bind:value={codec} disabled={starting}>
                <option value="h264">H.264</option>
                <option value="h265">H.265</option>
              </select>
            </label>
          </div>
        </div>

        <div class="card">
          <button
            type="button"
            class="primary-action"
            onclick={handleStart}
            disabled={!selectedDevice || starting || sources.length >= MAX_CONCURRENT_SOURCES}
          >
            {starting ? "Iniciando..." : "Iniciar transmissão"}
          </button>
          {#if sources.length >= MAX_CONCURRENT_SOURCES}
            <p class="hint">Limite de {MAX_CONCURRENT_SOURCES} fontes simultâneas atingido.</p>
          {/if}
        </div>
      {:else}
        <div class="card">
          <RtspPanel
            activeIds={rtspActiveIds}
            onError={applyError}
            onStarted={handleRtspStarted}
            onStopped={handleRtspStopped}
          />
        </div>
      {/if}
    </aside>

    <section class="main-col">
      {#if errorMsg}
        <div class="error-banner">
          <p>{errorMsg}</p>
          {#if actionHint}
            <p class="hint">{actionHint}</p>
          {/if}
        </div>
      {/if}

      <div class="card">
        <h2>Fontes ativas</h2>
        {#if sources.length === 0}
          <p class="empty">
            Nenhuma fonte ativa. Selecione um dispositivo Android ou uma câmera
            RTSP na barra lateral e inicie a transmissão.
          </p>
        {/if}
        <SourceGrid
          {sources}
          {expandedId}
          onSelect={(id) => (expandedId = id)}
          onStop={handleStopSource}
          onRename={(source, name) => handleRenameDevice(source.serial!, name)}
        />
      </div>

      {#if expandedSource}
        <div class="card expanded-card">
          <div class="expanded-header">
            <button type="button" class="back-link" onclick={() => (expandedId = null)}>
              ‹ Voltar à grade
            </button>
            <span class="expanded-name">{expandedSource.name}</span>
            {#if isSessionError(expandedSource.state)}
              <span class="expanded-status error">Erro: {expandedSource.state.error}</span>
            {:else}
              <span class="expanded-status">{expandedSource.state}</span>
            {/if}
          </div>

          <div class="preview-card">
            <div class="card-header">
              <span class="rec-dot small"></span>
              <h2>Preview ao vivo</h2>
              <span class="header-hint">Toque na imagem para focar</span>
            </div>
            <Preview
              sessionId={expandedSource.sessionId}
              onTap={expandedSource.kind === "android" ? handleTapToFocus : undefined}
            />
          </div>

          {#if expandedSource.kind === "android" && expandedSource.serial}
            <div class="subcard">
              <h2>Modo</h2>
              <ModeSelector
                sessionId={expandedSource.sessionId}
                serial={expandedSource.serial}
                mode={expandedSource.controlState?.mode ?? "auto"}
                onError={applyError}
              />
            </div>

            <div class="subcard">
              <h2>Controles da câmera</h2>
              <CameraControls
                sessionId={expandedSource.sessionId}
                serial={expandedSource.serial}
                mode={expandedSource.controlState?.mode ?? "auto"}
                onSessionChanged={(response) => adoptSession(expandedSource!.id, response)}
                onError={applyError}
              />
            </div>

            <div class="subcard">
              <h2>Captura RAW</h2>
              <RawPanel
                sessionId={expandedSource.sessionId}
                serial={expandedSource.serial}
                onError={applyError}
              />
            </div>
          {/if}
        </div>
      {/if}
    </section>
  </main>
</div>

<style>
  :root {
    font-family: Inter, Avenir, Helvetica, Arial, sans-serif;
    color: #14151a;
    background-color: #f6f6f4;
    --accent: #6d5cf5;
    --accent-hover: #5b4ae0;
    --card-bg: #ffffff;
    --card-border: rgba(20, 21, 26, 0.1);
    --muted: rgba(20, 21, 26, 0.6);
  }

  @media (prefers-color-scheme: dark) {
    :root {
      color: #f0f0f2;
      background-color: #202127;
      --card-bg: #2a2b33;
      --card-border: rgba(255, 255, 255, 0.08);
      --muted: rgba(240, 240, 242, 0.6);
    }
  }

  .app {
    min-height: 100vh;
    display: flex;
    flex-direction: column;
  }

  .topbar {
    display: flex;
    align-items: center;
    gap: 1rem;
    padding: 1rem 1.5rem;
    border-bottom: 1px solid var(--card-border);
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-weight: 700;
    font-size: 1.1rem;
  }

  .brand-dot {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: var(--accent);
  }

  .source-count {
    margin-left: auto;
    font-size: 0.8em;
    font-weight: 600;
    color: var(--muted);
    background: var(--card-bg);
    border: 1px solid var(--card-border);
    border-radius: 999px;
    padding: 0.35rem 0.8rem;
  }

  .layout {
    flex: 1;
    display: grid;
    grid-template-columns: 380px 1fr;
    gap: 1.5rem;
    padding: 1.5rem;
    max-width: 1600px;
    width: 100%;
    margin: 0 auto;
    align-items: start;
  }

  @media (max-width: 900px) {
    .layout {
      grid-template-columns: 1fr;
    }
  }

  .sidebar,
  .main-col {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .tabs {
    display: flex;
    gap: 0.4rem;
    padding: 0.3rem;
    background: var(--card-bg);
    border: 1px solid var(--card-border);
    border-radius: 10px;
  }

  .tabs button {
    flex: 1;
    padding: 0.5rem 0.75rem;
    border-radius: 8px;
    border: none;
    background: transparent;
    color: inherit;
    cursor: pointer;
    font-weight: 600;
    font-size: 0.85em;
  }

  .tabs button.active {
    background: var(--accent);
    color: white;
  }

  .card {
    background: var(--card-bg);
    border: 1px solid var(--card-border);
    border-radius: 12px;
    padding: 1.1rem;
  }

  .subcard {
    background: var(--card-bg);
    border: 1px solid var(--card-border);
    border-radius: 12px;
    padding: 1.1rem;
  }

  .card-header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-bottom: 0.75rem;
  }

  .card-header h2 {
    margin: 0;
  }

  .header-hint {
    margin-left: auto;
    font-size: 0.8em;
    color: var(--muted);
  }

  h2 {
    font-size: 0.95rem;
    margin: 0 0 0.75rem;
  }

  .empty {
    font-size: 0.85em;
    color: var(--muted);
    margin: 0 0 0.75rem;
  }

  .config-grid {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 0.75rem;
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    font-size: 0.85em;
    color: var(--muted);
  }

  select {
    padding: 0.45rem 0.6rem;
    border-radius: 8px;
    border: 1px solid var(--card-border);
    background: transparent;
    color: inherit;
    font-size: 0.9em;
  }

  .hint {
    font-size: 0.8em;
    color: var(--muted);
    margin: 0.6rem 0 0;
  }

  .primary-action {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    width: 100%;
    padding: 0.7rem 1rem;
    border-radius: 10px;
    border: none;
    background: var(--accent);
    color: white;
    cursor: pointer;
    font-weight: 600;
  }

  .primary-action:hover:not(:disabled) {
    background: var(--accent-hover);
  }

  .primary-action:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .rec-dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: white;
    animation: pulse 1.4s ease-in-out infinite;
  }

  .rec-dot.small {
    background: #dc2626;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.4;
    }
  }

  .preview-card :global(.preview) {
    max-width: none;
  }

  .expanded-card {
    display: flex;
    flex-direction: column;
    gap: 1rem;
    border-color: var(--accent);
  }

  .expanded-header {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .back-link {
    background: none;
    border: none;
    color: var(--muted);
    font-weight: 600;
    font-size: 0.8em;
    cursor: pointer;
    padding: 0;
  }

  .expanded-name {
    font-weight: 700;
    font-size: 1rem;
  }

  .expanded-status {
    margin-left: auto;
    font-size: 0.8em;
    color: var(--muted);
    text-transform: capitalize;
  }

  .expanded-status.error {
    color: #dc2626;
  }

  .error-banner {
    background: rgba(220, 38, 38, 0.1);
    border: 1px solid rgba(220, 38, 38, 0.3);
    border-radius: 8px;
    padding: 0.6rem 0.8rem;
    color: #b91c1c;
  }

  .error-banner .hint {
    margin: 0.3rem 0 0;
    opacity: 0.85;
    color: inherit;
  }

  .error-banner p {
    margin: 0;
  }
</style>
