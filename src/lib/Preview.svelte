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
    // Troca de sessão (ou volta a inativo): descarta o frame antigo, senão
    // fica mostrando uma imagem congelada de uma sessão que já acabou.
    frameSrc = null;

    if (!sessionId) {
      return;
    }

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
