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
  }: {
    selectedSerial?: string | null;
    onSelect?: (device: AndroidDevice) => void;
  } = $props();

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
        <button
          type="button"
          class="device-row"
          onclick={() => select(device)}
          disabled={device.auth_state !== "authorized" || !device.compatible}
        >
          <span class="model">{device.model}</span>
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
        </button>

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

  .device-row:disabled {
    cursor: not-allowed;
  }

  .model {
    font-weight: 600;
    flex: 1;
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
