<script lang="ts">
  import { onDestroy } from "svelte";
  import { onPreviewFrame } from "./api";

  let {
    sessionId,
    onTap,
  }: {
    sessionId: string | null;
    /** Tap-to-focus (US2): coordenadas normalizadas [0,1] do clique. */
    onTap?: (x: number, y: number) => void;
  } = $props();

  function handleClick(event: MouseEvent) {
    if (!onTap || !frameSrc) return;
    const target = event.currentTarget as HTMLElement;
    const rect = target.getBoundingClientRect();
    const x = (event.clientX - rect.left) / rect.width;
    const y = (event.clientY - rect.top) / rect.height;
    if (x >= 0 && x <= 1 && y >= 0 && y <= 1) {
      onTap(x, y);
    }
  }

  let frameSrc = $state<string | null>(null);
  let unlisten: (() => void) | null = null;

  $effect(() => {
    if (!sessionId) {
      // Sessão acabou de verdade: descarta o frame, senão fica mostrando uma
      // imagem congelada de algo que não está mais transmitindo.
      frameSrc = null;
      return;
    }

    // Numa TROCA de sessão (giro/espelho/switch_camera, que no Linux sempre
    // reiniciam a sessão — lib.rs `set_orientation`) o frame antigo é mantido
    // de propósito: é a mesma câmera física, e limpar aqui era metade do
    // "preview piscando" (D2, 2026-08-12). O outro meio segundo vinha do
    // read-back do v4l2 (D1): com o OBS segurando o device, o primeiro frame
    // da sessão nova pode demorar, e apagar antes deixava o placeholder
    // "Aguardando primeiro frame" pendurado no lugar de uma imagem válida.
    // Quem sinaliza que a sessão não está saudável é o status do card, não o
    // sumiço do preview.

    const currentSession = sessionId;
    let cancelled = false;
    onPreviewFrame((event) => {
      if (cancelled || event.session_id !== currentSession) return;
      frameSrc = `data:image/jpeg;base64,${event.jpeg_base64}`;
    }).then((fn) => {
      if (cancelled) {
        fn();
      } else {
        unlisten = fn;
      }
    });

    return () => {
      cancelled = true;
      unlisten?.();
      unlisten = null;
    };
  });

  onDestroy(() => {
    unlisten?.();
  });
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div
  class="preview"
  class:tappable={onTap !== undefined && frameSrc !== null}
  onclick={handleClick}
>
  {#if frameSrc}
    <img src={frameSrc} alt="Preview do stream" />
  {:else}
    <div class="placeholder">
      <p>{sessionId ? "Aguardando primeiro frame..." : "Aguardando stream"}</p>
    </div>
  {/if}
</div>

<style>
  .preview {
    aspect-ratio: 16 / 9;
    width: 100%;
    background: #111;
    border-radius: 8px;
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .preview.tappable {
    cursor: crosshair;
  }

  .preview img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }

  .placeholder {
    color: #888;
    font-size: 0.9em;
    text-align: center;
    padding: 1rem;
  }
</style>
