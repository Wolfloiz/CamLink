<script lang="ts">
  import { onDestroy } from "svelte";
  import CameraControls from "$lib/CameraControls.svelte";
  import DeviceList from "$lib/DeviceList.svelte";
  import Preview from "$lib/Preview.svelte";
  import RtspPanel from "$lib/RtspPanel.svelte";
  import {
    onSessionReplaced,
    onSessionState,
    setControl,
    startStream,
    stopStream,
  } from "$lib/api";
  import {
    DEFAULT_STREAM_CONFIG,
    isSessionActive,
    isSessionError,
    type AndroidDevice,
    type SessionState,
    type SessionStats,
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

  let sessionId = $state<string | null>(null);
  let sessionState = $state<SessionState | null>(null);
  let sessionStats = $state<SessionStats | null>(null);
  let starting = $state(false);
  let stopping = $state(false);
  let errorMsg = $state<string | null>(null);
  let actionHint = $state<string | null>(null);

  const active = $derived(sessionId !== null);

  // Sessão RTSP ativa mais recente (preview independente da sessão Android).
  let rtspSessionId = $state<string | null>(null);

  let unlistenSessionState: (() => void) | null = null;
  let unlistenSessionReplaced: (() => void) | null = null;

  $effect(() => {
    onSessionState((event) => {
      if (event.session_id !== sessionId) return;
      sessionState = event.state;
      sessionStats = event.stats;
      if (event.state === "idle") {
        sessionId = null;
      }
    }).then((fn) => {
      unlistenSessionState = fn;
    });
    // Rotação 90°/270° reinicia a sessão por baixo (T079) — realinha o
    // session_id sem o usuário perceber.
    onSessionReplaced((event) => {
      if (event.old_session_id !== sessionId) return;
      sessionId = event.response.session_id;
      sessionState = event.response.state;
      sessionStats = event.response.stats;
    }).then((fn) => {
      unlistenSessionReplaced = fn;
    });
    return () => {
      unlistenSessionState?.();
      unlistenSessionState = null;
      unlistenSessionReplaced?.();
      unlistenSessionReplaced = null;
    };
  });

  onDestroy(() => {
    unlistenSessionState?.();
    unlistenSessionReplaced?.();
  });

  function adoptSession(response: StartStreamResponse) {
    sessionId = response.session_id;
    sessionState = response.state;
    sessionStats = response.stats;
  }

  async function handleTapToFocus(x: number, y: number) {
    if (!sessionId) return;
    try {
      await setControl(sessionId, { focus: { tap: { x, y } } });
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
    errorMsg = null;
    actionHint = null;
    starting = true;
    try {
      const response = await startStream(selectedDevice.serial, buildConfig());
      sessionId = response.session_id;
      sessionState = response.state;
      sessionStats = response.stats;
    } catch (e) {
      applyError(e);
    } finally {
      starting = false;
    }
  }

  async function handleStop() {
    if (!sessionId) return;
    stopping = true;
    try {
      await stopStream(sessionId);
    } catch (e) {
      applyError(e);
    } finally {
      sessionId = null;
      sessionState = null;
      sessionStats = null;
      stopping = false;
    }
  }
</script>

<main class="page">
  <h1>CamLink</h1>

  <section>
    <h2>Dispositivos</h2>
    <DeviceList
      selectedSerial={selectedDevice?.serial ?? null}
      onSelect={(device) => (selectedDevice = device)}
    />
  </section>

  <section class="config">
    <h2>Configuração</h2>
    <div class="config-grid">
      <label>
        Resolução
        <select bind:value={resolutionIndex} disabled={active}>
          {#each RESOLUTIONS as res, i (i)}
            <option value={i}>{res[0]}x{res[1]}</option>
          {/each}
        </select>
      </label>

      <label>
        FPS
        <select bind:value={fps} disabled={active}>
          {#each FPS_OPTIONS as f (f)}
            <option value={f}>{f}</option>
          {/each}
        </select>
      </label>

      <label>
        Bitrate (Mbps)
        <input
          type="number"
          min="1"
          max="50"
          step="1"
          bind:value={bitrateMbps}
          disabled={active}
        />
      </label>

      <label>
        Codec
        <select bind:value={codec} disabled={active}>
          <option value="h264">H.264</option>
          <option value="h265">H.265</option>
        </select>
      </label>
    </div>
  </section>

  <section class="controls">
    {#if !active}
      <button type="button" onclick={handleStart} disabled={!selectedDevice || starting}>
        {starting ? "Iniciando..." : "Iniciar"}
      </button>
    {:else}
      <button type="button" onclick={handleStop} disabled={stopping}>
        {stopping ? "Parando..." : "Parar"}
      </button>
    {/if}

    {#if sessionState}
      <span class="status">
        {#if isSessionError(sessionState)}
          Erro: {sessionState.error}
        {:else}
          {sessionState}
        {/if}
      </span>
    {/if}

    {#if sessionStats && sessionState && isSessionActive(sessionState)}
      <span class="stats">
        {sessionStats.fps.toFixed(1)} fps · {sessionStats.reconnects} reconexões
      </span>
    {/if}
  </section>

  {#if errorMsg}
    <div class="error-banner">
      <p>{errorMsg}</p>
      {#if actionHint}
        <p class="hint">{actionHint}</p>
      {/if}
    </div>
  {/if}

  {#if active && sessionId && selectedDevice}
    <section>
      <h2>Controles da câmera</h2>
      <CameraControls
        {sessionId}
        serial={selectedDevice.serial}
        onSessionChanged={adoptSession}
        onError={applyError}
      />
    </section>
  {/if}

  <section>
    <h2>Preview</h2>
    <Preview
      sessionId={active ? sessionId : rtspSessionId}
      onTap={active ? handleTapToFocus : undefined}
    />
  </section>

  <section>
    <h2>Câmeras IP (RTSP)</h2>
    <RtspPanel
      onError={applyError}
      onStarted={(_id, response) => (rtspSessionId = response.session_id)}
    />
  </section>
</main>

<style>
  :root {
    font-family: Inter, Avenir, Helvetica, Arial, sans-serif;
    color: #0f0f0f;
    background-color: #f6f6f6;
  }

  @media (prefers-color-scheme: dark) {
    :root {
      color: #f6f6f6;
      background-color: #2f2f2f;
    }
  }

  .page {
    max-width: 640px;
    margin: 0 auto;
    padding: 1.5rem;
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
  }

  h1 {
    margin: 0;
  }

  h2 {
    font-size: 1rem;
    margin: 0 0 0.5rem;
    opacity: 0.8;
  }

  .config-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(140px, 1fr));
    gap: 0.75rem;
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    font-size: 0.85em;
  }

  .controls {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    flex-wrap: wrap;
  }

  button {
    padding: 0.5rem 1.2rem;
    border-radius: 6px;
    border: none;
    background: #4a8cff;
    color: white;
    cursor: pointer;
    font-weight: 600;
  }

  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .status {
    font-size: 0.85em;
    text-transform: capitalize;
    opacity: 0.8;
  }

  .stats {
    font-size: 0.8em;
    opacity: 0.6;
  }

  .error-banner {
    background: rgba(220, 38, 38, 0.1);
    border: 1px solid rgba(220, 38, 38, 0.3);
    border-radius: 6px;
    padding: 0.6rem 0.8rem;
    color: #b91c1c;
  }

  .error-banner .hint {
    margin: 0.3rem 0 0;
    opacity: 0.85;
  }

  .error-banner p {
    margin: 0;
  }
</style>
