<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import {
    listDevices,
    onDeviceConnected,
    onDeviceDisconnected,
    onDeviceUnauthorized,
  } from "./api";
  import type { AndroidDevice } from "./types";

  let {
    selectedSerial = null,
    onSelect,
    nicknames = {},
    onRename,
  }: {
    selectedSerial?: string | null;
    onSelect?: (device: AndroidDevice) => void;
    /** T065e: apelido por serial — sobrescreve `device.model` na exibição. */
    nicknames?: Record<string, string>;
    onRename?: (serial: string, name: string) => void;
  } = $props();

  // T065e: editar o apelido ANTES de iniciar a fonte é o caminho sem
  // restart — uma vez que o device virtual existe, o v4l2loopback não tem
  // como renomeá-lo (só add/delete), então um rename ao vivo só reflete no
  // OBS na próxima vez que a fonte for reiniciada.
  let renamingSerial = $state<string | null>(null);
  let draftName = $state("");

  function startRename(serial: string, currentName: string, event: MouseEvent) {
    event.stopPropagation();
    draftName = currentName;
    renamingSerial = serial;
  }

  function commitRename(serial: string, event?: Event) {
    event?.stopPropagation();
    renamingSerial = null;
    const trimmed = draftName.trim();
    onRename?.(serial, trimmed);
  }

  function cancelRename(event: Event) {
    event.stopPropagation();
    renamingSerial = null;
  }

  let devices = $state<AndroidDevice[]>([]);
  let loadError = $state<string | null>(null);
  const unlisteners: Array<() => void> = [];

  function upsert(device: AndroidDevice) {
    const idx = devices.findIndex((d) => d.serial === device.serial);
    if (idx >= 0) {
      devices = devices.map((d) => (d.serial === device.serial ? device : d));
    } else {
      devices = [...devices, device];
    }
  }

  onMount(() => {
    listDevices()
      .then((list) => {
        devices = list;
      })
      .catch((e) => {
        loadError = String(e);
      });

    (async () => {
      unlisteners.push(await onDeviceConnected((device) => upsert(device)));
      unlisteners.push(
        await onDeviceDisconnected((serial) => {
          devices = devices.filter((d) => d.serial !== serial);
          if (selectedSerial === serial) {
            selectedSerial = null;
          }
        }),
      );
      unlisteners.push(
        await onDeviceUnauthorized((serial) => {
          upsert({
            serial,
            model: serial,
            auth_state: "unauthorized",
            compatible: true,
            incompat_reason: null,
            capabilities: null,
          });
        }),
      );
    })();
  });

  onDestroy(() => {
    for (const unlisten of unlisteners) unlisten();
  });

  function select(device: AndroidDevice) {
    if (device.auth_state !== "authorized" || !device.compatible) return;
    selectedSerial = device.serial;
    onSelect?.(device);
  }
</script>

<div class="device-list">
  {#if loadError}
    <p class="error">Falha ao listar dispositivos: {loadError}</p>
  {/if}

  {#if devices.length === 0 && !loadError}
    <p class="empty">
      Nenhum dispositivo Android detectado. Conecte o celular via USB com a
      depuração USB habilitada.
    </p>
  {/if}

  <ul>
    {#each devices as device (device.serial)}
      <li
        class="device"
        class:selected={selectedSerial === device.serial}
        class:disabled={device.auth_state !== "authorized" ||
          !device.compatible}
      >
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
        <div
          class="device-row"
          class:device-row-disabled={device.auth_state !== "authorized" || !device.compatible}
          onclick={() => select(device)}
          role="button"
          tabindex="0"
        >
          {#if renamingSerial === device.serial}
            <!-- svelte-ignore a11y_autofocus -->
            <input
              class="name-input"
              type="text"
              bind:value={draftName}
              autofocus
              onclick={(e) => e.stopPropagation()}
              onkeydown={(e) => {
                if (e.key === "Enter") commitRename(device.serial, e);
                if (e.key === "Escape") cancelRename(e);
              }}
              onblur={() => commitRename(device.serial)}
            />
          {:else}
            <span class="model">{nicknames[device.serial] || device.model}</span>
            <button
              type="button"
              class="rename-btn"
              title="Renomear"
              onclick={(e) => startRename(device.serial, nicknames[device.serial] ?? "", e)}
            >
              ✎
            </button>
          {/if}
          <span class="serial">{device.serial}</span>
          {#if !device.compatible}
            <span class="badge badge-incompatible"><span class="dot"></span>Incompatível</span>
          {:else}
            <span class="badge badge-{device.auth_state}">
              <span class="dot"></span>
              {#if device.auth_state === "authorized"}
                Autorizado
              {:else if device.auth_state === "unauthorized"}
                Não autorizado
              {:else}
                Offline
              {/if}
            </span>
          {/if}
        </div>

        {#if device.auth_state === "unauthorized"}
          <div class="guide">
            <p><strong>Autorize a depuração USB no celular:</strong></p>
            <ol>
              <li>Olhe a tela do celular — deve aparecer um popup pedindo autorização.</li>
              <li>Marque "Sempre permitir deste computador".</li>
              <li>Toque em "Permitir".</li>
              <li>Se o popup não aparecer, desconecte e reconecte o cabo USB.</li>
            </ol>
          </div>
        {/if}

        {#if !device.compatible && device.incompat_reason}
          <p class="incompatible">{device.incompat_reason}</p>
        {/if}
      </li>
    {/each}
  </ul>
</div>

<style>
  .device-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .device {
    border: 1px solid var(--border-color, #ccc);
    border-radius: 10px;
    overflow: hidden;
    transition: border-color 0.15s ease, background 0.15s ease;
  }

  .device.selected {
    border-color: #6d5cf5;
    background: rgba(109, 92, 245, 0.08);
  }

  .device.disabled {
    opacity: 0.6;
  }

  .device-row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    width: 100%;
    padding: 0.6rem 0.8rem;
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
    font: inherit;
    color: inherit;
  }

  .device-row-disabled {
    cursor: not-allowed;
  }

  .model {
    font-weight: 600;
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .rename-btn {
    border: none;
    background: transparent;
    color: inherit;
    opacity: 0.6;
    cursor: pointer;
    font-size: 0.85em;
    padding: 0 0.15rem;
    line-height: 1;
    flex-shrink: 0;
  }

  .rename-btn:hover {
    opacity: 1;
    color: #6d5cf5;
  }

  .name-input {
    flex: 1;
    min-width: 0;
    font-weight: 600;
    font-family: inherit;
    font-size: 1em;
    background: var(--card-bg, #fff);
    color: inherit;
    border: 1px solid #6d5cf5;
    border-radius: 6px;
    padding: 0.1rem 0.4rem;
  }

  .serial {
    opacity: 0.6;
    font-size: 0.85em;
  }

  .badge {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    font-size: 0.75em;
    font-weight: 600;
    padding: 0.2rem 0.6rem;
    border-radius: 999px;
    white-space: nowrap;
  }

  .badge .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: currentColor;
    flex-shrink: 0;
  }

  .badge-authorized {
    background: rgba(34, 197, 94, 0.15);
    color: #15803d;
  }

  .badge-unauthorized {
    background: rgba(217, 119, 6, 0.15);
    color: #b45309;
  }

  .badge-offline {
    background: rgba(107, 114, 128, 0.15);
    color: #4b5563;
  }

  .badge-incompatible {
    background: rgba(107, 114, 128, 0.15);
    color: #6b7280;
  }

  .guide,
  .incompatible {
    padding: 0.6rem 0.8rem 0.8rem;
    font-size: 0.85em;
    background: rgba(217, 119, 6, 0.08);
  }

  .guide ol {
    margin: 0.3rem 0 0;
    padding-left: 1.2rem;
  }

  .incompatible {
    background: rgba(220, 38, 38, 0.08);
    color: #b91c1c;
  }

  .error {
    color: #b91c1c;
  }

  .empty {
    opacity: 0.7;
  }
</style>
