<script lang="ts">
  // T040/T081 — Controles de câmera em runtime (US2/FR-008..016a).
  // Habilitação ESTRITAMENTE por capabilities (FR-016): nada é assumido por
  // modelo de aparelho; controle não suportado aparece desabilitado com a
  // explicação. Girar/espelhar são locais (não dependem de capability).
  import { getCapabilities, setControl, switchCamera } from "./api";
  import type {
    ControlChange,
    ControlState,
    DeviceCapabilities,
    Rotation,
    StartStreamResponse,
    WbMode,
  } from "./types";

  let {
    sessionId,
    serial,
    onSessionChanged,
    onError,
  }: {
    sessionId: string;
    serial: string;
    /** switch_camera / girar 90°-270° reiniciam a sessão → novo session_id. */
    onSessionChanged: (response: StartStreamResponse) => void;
    onError: (e: unknown) => void;
  } = $props();

  const ROTATIONS: Rotation[] = ["deg0", "deg90", "deg180", "deg270"];
  const ROTATION_LABEL: Record<Rotation, string> = {
    deg0: "0°",
    deg90: "90°",
    deg180: "180°",
    deg270: "270°",
  };
  const WB_LABEL: Record<WbMode, string> = {
    auto: "Automático",
    daylight: "Luz do dia",
    cloudy: "Nublado",
    fluorescent: "Fluorescente",
    incandescent: "Incandescente",
  };

  let caps = $state<DeviceCapabilities | null>(null);
  let capsError = $state<string | null>(null);
  let busy = $state(false);

  let zoom = $state(1.0);
  let exposureComp = $state(0);
  let wbMode = $state<WbMode>("auto");
  let eis = $state(true);
  let torch = $state(false);
  let rotation = $state<Rotation>("deg0");
  let mirror = $state(false);

  $effect(() => {
    // Recarrega quando a sessão muda (switch/rotate criam sessão nova).
    void sessionId;
    caps = null;
    capsError = null;
    getCapabilities(serial)
      .then((c) => (caps = c))
      .catch((e) => {
        capsError =
          e && typeof e === "object" && "msg" in e
            ? String((e as { msg: string }).msg)
            : String(e);
      });
  });

  const hasBothFacings = $derived(
    caps !== null &&
      caps.cameras.some((c) => c.facing === "front") &&
      caps.cameras.some((c) => c.facing === "back"),
  );

  async function apply(change: ControlChange) {
    busy = true;
    try {
      await setControl(sessionId, change);
    } catch (e) {
      onError(e);
    } finally {
      busy = false;
    }
  }

  async function handleFlip() {
    if (!caps) return;
    const current = caps.cameras.find((c) => c.id === currentCameraId());
    const targetFacing = current?.facing === "front" ? "back" : "front";
    const target = caps.cameras.find((c) => c.facing === targetFacing);
    if (!target) return;
    busy = true;
    try {
      const response = await switchCamera(sessionId, target.id);
      cameraId = target.id;
      onSessionChanged(response);
    } catch (e) {
      onError(e);
    } finally {
      busy = false;
    }
  }

  let cameraId = $state("0");
  function currentCameraId(): string {
    return cameraId;
  }

  async function handleRotate() {
    // Quando a rotação troca o lado (paisagem↔retrato), o backend reinicia
    // a sessão e emite `session_replaced` com o novo session_id — a página
    // escuta esse evento e realinha; aqui só disparamos o comando.
    const next = ROTATIONS[(ROTATIONS.indexOf(rotation) + 1) % ROTATIONS.length];
    rotation = next;
    await apply({ rotation: next });
  }

  async function handleMirror() {
    mirror = !mirror;
    await apply({ mirror });
  }
</script>

<div class="camera-controls">
  {#if capsError}
    <p class="caps-error">
      Controles indisponíveis: {capsError}
    </p>
  {:else if !caps}
    <p class="loading">Consultando capacidades do aparelho...</p>
  {:else}
    <div class="grid">
      <label>
        Zoom ({zoom.toFixed(1)}x)
        <input
          type="range"
          min={caps.zoom_range[0]}
          max={caps.zoom_range[1]}
          step="0.1"
          bind:value={zoom}
          onchange={() => apply({ zoom })}
          disabled={busy}
        />
      </label>

      <label>
        Exposição ({exposureComp >= 0 ? "+" : ""}{exposureComp} EV)
        <input
          type="range"
          min={caps.exposure_comp_range[0]}
          max={caps.exposure_comp_range[1]}
          step="1"
          bind:value={exposureComp}
          onchange={() => apply({ exposure_comp: exposureComp })}
          disabled={busy}
        />
      </label>

      <label>
        Balanço de branco
        <select
          bind:value={wbMode}
          onchange={() => apply({ wb: wbMode })}
          disabled={busy || caps.wb_modes.length <= 1}
        >
          {#each caps.wb_modes as mode (mode)}
            <option value={mode}>{WB_LABEL[mode]}</option>
          {/each}
        </select>
      </label>

      <label
        class:unsupported={caps.iso_range === null}
        title={caps.iso_range === null
          ? "Este aparelho não expõe ISO manual"
          : "ISO manual exige o modo Pro (chega com os modos inteligentes)"}
      >
        ISO {caps.iso_range
          ? `(${caps.iso_range[0]}–${caps.iso_range[1]})`
          : "(não suportado)"}
        <input type="number" disabled value={caps.iso_range?.[0] ?? 0} />
      </label>
    </div>

    <div class="toggles">
      <button
        type="button"
        disabled={busy || !caps.supports_eis}
        class:active={eis}
        title={caps.supports_eis
          ? "Estabilização eletrônica"
          : "EIS não suportado neste aparelho"}
        onclick={() => {
          eis = !eis;
          apply({ eis });
        }}
      >
        EIS {eis ? "on" : "off"}
      </button>

      <button
        type="button"
        disabled={busy || !caps.supports_torch}
        class:active={torch}
        title={caps.supports_torch
          ? "Lanterna do celular"
          : "Lanterna não suportada neste aparelho"}
        onclick={() => {
          torch = !torch;
          apply({ torch });
        }}
      >
        Lanterna {torch ? "on" : "off"}
      </button>

      <button
        type="button"
        disabled={busy || !hasBothFacings}
        title={hasBothFacings
          ? "Alternar frontal/traseira (≤ 2 s) — se o Meet mostrar a câmera como indisponível depois, dê F5 na aba"
          : "Aparelho sem par frontal+traseira"}
        onclick={handleFlip}
      >
        Frontal/Traseira
      </button>

      <button
        type="button"
        disabled={busy}
        title="Girar o vídeo entregue em passos de 90° — no Linux reinicia a sessão sempre; no Windows só ao girar 90°/270°; se o Meet mostrar a câmera como indisponível depois, dê F5 na aba"
        onclick={handleRotate}
      >
        Girar ({ROTATION_LABEL[rotation]})
      </button>

      <button
        type="button"
        disabled={busy}
        class:active={mirror}
        title="Espelhar horizontalmente o vídeo entregue — no Linux reinicia a sessão (limitação do pipeline v4l2)"
        onclick={handleMirror}
      >
        Espelhar {mirror ? "on" : "off"}
      </button>
    </div>

    <p class="hint">
      Toque no preview para focar naquela região (tap-to-focus).
    </p>
    <p class="hint">
      Trocar de câmera, espelhar ou girar reinicia a sessão no Linux (no
      Windows só ao girar 90°/270°) — se o app de chamada (Meet etc.)
      mostrar a câmera como indisponível depois, dê F5 na aba ou saia e
      entre de novo na chamada.
    </p>
  {/if}
</div>

<style>
  .camera-controls {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
    gap: 0.75rem;
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    font-size: 0.85em;
  }

  label.unsupported {
    opacity: 0.5;
  }

  .toggles {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .toggles button {
    padding: 0.35rem 0.8rem;
    border-radius: 6px;
    border: 1px solid rgba(120, 120, 120, 0.4);
    background: transparent;
    color: inherit;
    cursor: pointer;
    font-size: 0.85em;
  }

  .toggles button.active {
    background: #4a8cff;
    border-color: #4a8cff;
    color: white;
  }

  .toggles button:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .hint,
  .loading,
  .caps-error {
    font-size: 0.8em;
    opacity: 0.7;
    margin: 0;
  }

  .caps-error {
    color: #b91c1c;
    opacity: 1;
  }
</style>
