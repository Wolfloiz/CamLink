// Preferência de tema da interface (T083).
//
// O app sempre soube renderizar claro e escuro, mas só obedecendo o
// `prefers-color-scheme` do sistema — o usuário não tinha como escolher.
// Aqui a escolha vira explícita e persistida.
//
// O atributo `data-theme` no <html> é o contrato com o CSS de
// `+page.svelte`: ausente = seguir o SO; "light"/"dark" = fixado.

export type ThemePreference = "light" | "dark" | "system";

/** Chave no localStorage. Espelhada no script anti-flash de `app.html` — se
 * mudar aqui, mudar lá. */
export const STORAGE_KEY = "camlink:theme";

function isPreference(value: unknown): value is ThemePreference {
  return value === "light" || value === "dark" || value === "system";
}

/** Preferência salva, ou "system" se não houver nenhuma. */
export function readPreference(): ThemePreference {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    return isPreference(saved) ? saved : "system";
  } catch {
    // localStorage pode lançar (janela privada, storage bloqueado por
    // política). Sem preferência salva o app segue o SO, que é o
    // comportamento que ele já tinha.
    return "system";
  }
}

/** Aplica no <html> e persiste. */
export function setPreference(pref: ThemePreference): void {
  const root = document.documentElement;
  if (pref === "system") {
    root.removeAttribute("data-theme");
  } else {
    root.setAttribute("data-theme", pref);
  }
  try {
    localStorage.setItem(STORAGE_KEY, pref);
  } catch {
    // Não persistir não impede a sessão atual de funcionar.
  }
}
