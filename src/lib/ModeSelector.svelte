<script lang="ts">
  // US3 — Seletor de modos inteligentes (Auto/Night/Sport/Pro, FR US3).
  // Aplica via set_control({mode}); estado ativo vem do evento control_state
  // (ver +page.svelte), não de estado local — outros clientes/sessões podem
  // mudar o modo por fora.
  import { getCapabilities, setControl } from "./api";
  import type { DeviceCapabilities, SmartMode } from "./types";

  let {
    sessionId,
    serial,
    mode,
    onError,
  }: {
    sessionId: string;
    serial: string;
    mode: SmartMode;
    onError: (e: unknown) => void;
  } = $props();

  const MODES: SmartMode[] = ["auto", "night", "sport", "pro"];
  const MODE_LABEL: Record<SmartMode, string> = {
    auto: "Auto",
    night: "Night",
    sport: "Sport",
    pro: "Pro",
  };

  // fps alvo por modo (control-protocol.md, tabela de modos); pro é livre
  // (não seta AE_TARGET_FPS_RANGE).
  const MODE_TARGET_FPS: Partial<Record<SmartMode, [number, number]>> = {
    auto: [30, 30],
    night: [15, 30],
    sport: [60, 60],
  };

  let busy = $state(false);
  let caps = $state<DeviceCapabilities | null>(null);

  $effect(() => {
    // Recarrega quando a sessão muda (switch/rotate criam sessão nova).
    void sessionId;
    caps = null;
    getCapabilities(serial).then((c) => (caps = c));
  });

  // Mesma lógica de `closestSupportedFpsRange` do fork
  // (CamLinkCameraController.java): entre os ranges que o aparelho declara
  // em CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES, escolhe o mais próximo do
  // alvo do modo. Pedir um range não declarado é comportamento indefinido
  // no Camera2 (visto em bancada: AE "patina" e a fps real oscila) — por
  // isso o fork já faz esse clamp; aqui só refletimos pro usuário o
  // resultado, pra nunca deixar "Sport" implicando 60fps quando o aparelho
  // não suporta (spec.md, edge case "nunca falha silenciosa").
  function closestSupportedFpsRange(
    available: Array<[number, number]>,
    want: [number, number],
  ): [number, number] | null {
    if (available.length === 0) return null;
    let best = available[0];
    let bestScore = Infinity;
    for (const r of available) {
      const score = Math.abs(r[0] - want[0]) + Math.abs(r[1] - want[1]);
      if (score < bestScore) {
        bestScore = score;
        best = r;
      }
    }
    return best;
  }

  function fpsNote(m: SmartMode): string | null {
    const want = MODE_TARGET_FPS[m];
    if (!want || !caps || caps.cameras.length === 0) return null;
    const available = caps.cameras[0].fps_ranges;
    const got = closestSupportedFpsRange(available, want);
    if (!got || (got[0] === want[0] && got[1] === want[1])) return null;
    return got[0] === got[1] ? `${got[0]}fps` : `${got[0]}–${got[1]}fps`;
  }

  async function selectMode(target: SmartMode) {
    if (target === mode || busy) return;
    busy = true;
    try {
      await setControl(sessionId, { mode: target });
    } catch (e) {
      onError(e);
    } finally {
      busy = false;
    }
  }
</script>

<div class="toggles">
  {#each MODES as m (m)}
    {@const note = fpsNote(m)}
    <button
      type="button"
      disabled={busy}
      class:active={mode === m}
      title={note
        ? `Este aparelho não declara ${MODE_TARGET_FPS[m]![0] === MODE_TARGET_FPS[m]![1] ? `${MODE_TARGET_FPS[m]![0]}fps` : `${MODE_TARGET_FPS[m]![0]}–${MODE_TARGET_FPS[m]![1]}fps`} para a câmera — será usado o suportado mais próximo (${note})`
        : undefined}
      onclick={() => selectMode(m)}
    >
      {MODE_LABEL[m]}{#if note}
        <span class="fps-note">({note})</span>
      {/if}
    </button>
  {/each}
</div>

<style>
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
    background: #6d5cf5;
    border-color: #6d5cf5;
    color: white;
  }

  .toggles button:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .fps-note {
    opacity: 0.75;
    font-size: 0.9em;
  }
</style>
