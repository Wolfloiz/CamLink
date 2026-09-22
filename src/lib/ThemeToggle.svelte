<script lang="ts">
  // Seletor de tema (T083). Claro / Escuro / Sistema, persistido.
  import { readPreference, setPreference, type ThemePreference } from "./theme";

  // O atributo `data-theme` já foi aplicado pelo script de `app.html` antes
  // do primeiro paint; aqui só espelhamos o valor salvo no estado da UI.
  let pref = $state<ThemePreference>(readPreference());

  const OPTIONS: { value: ThemePreference; label: string; icon: string }[] = [
    { value: "light", label: "Claro", icon: "☀" },
    { value: "dark", label: "Escuro", icon: "☽" },
    { value: "system", label: "Sistema", icon: "◐" },
  ];

  function choose(value: ThemePreference) {
    pref = value;
    setPreference(value);
  }
</script>

<div class="theme-toggle" role="group" aria-label="Tema da interface">
  {#each OPTIONS as opt (opt.value)}
    <button
      type="button"
      class:active={pref === opt.value}
      aria-pressed={pref === opt.value}
      title="Tema: {opt.label}"
      onclick={() => choose(opt.value)}
    >
      <span class="icon" aria-hidden="true">{opt.icon}</span>
      <span class="label">{opt.label}</span>
    </button>
  {/each}
</div>

<style>
  .theme-toggle {
    display: flex;
    gap: 0.2rem;
    padding: 0.2rem;
    background: var(--card-bg);
    border: 1px solid var(--card-border);
    border-radius: 0.625rem;
  }

  button {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.3rem 0.6rem;
    border: none;
    border-radius: 0.5rem;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    font: inherit;
    font-size: 0.8rem;
    font-weight: 600;
  }

  button:hover:not(.active) {
    color: var(--text);
  }

  button.active {
    background: var(--accent);
    color: #fff;
  }

  .icon {
    line-height: 1;
  }

  /* Em janela estreita sobram só os ícones — o seletor divide a topbar com
     a marca e o contador de fontes. */
  @media (max-width: 34rem) {
    .label {
      display: none;
    }
  }
</style>
