# Contribuindo com o CamLink

Projeto GPL-3.0. `main` é protegida: toda mudança entra por Pull Request,
com CI verde (fmt/clippy/test em Linux e Windows) e pelo menos 1 aprovação.

## Fluxo

1. Abra uma issue (ou pegue uma existente) antes de começar algo grande —
   evita dois devs/agentes trabalhando na mesma coisa ao mesmo tempo.
   Comente na issue que você vai pegá-la.
2. Branch a partir de `main`: `feat/nome-curto`, `fix/nome-curto`,
   `docs/nome-curto`.
3. Siga o TDD e as regras da [constituição](.specify/memory/constitution.md):
   testes antes da implementação, paridade Linux+Windows por feature
   (platform-specific só em `src-tauri/src/virtualcam/`), sem `unwrap()` em
   caminho falível, `// SAFETY:` em todo `unsafe` de FFI.
4. Antes de abrir o PR, rode localmente:
   ```
   cargo fmt --check
   cargo clippy -- -D warnings
   cargo test
   ```
   (dentro de `src-tauri/`) — são os gates que o CI também roda.
5. Abra o PR preenchendo o template. Se fechar uma task de
   `specs/001-phone-webcam-bridge/tasks.md`, atualize o checkbox e a nota
   de validação lá também.
6. **Avaliação de performance (fps/latência) só vale em build release**
   (`cargo build --release` / `cargo tauri build`) — `cargo tauri dev` usa
   o perfil dev, que não reflete desempenho real.

## Trabalhando em paralelo (múltiplos devs/agentes)

- Issues têm dono: comente antes de começar, pra não duplicar trabalho.
- PRs pequenos e focados mergeiam mais rápido e colidem menos entre si.
- Se duas issues tocam o mesmo arquivo/módulo, prefira sequenciar (uma
  espera a outra mergear) em vez de branches longas divergindo.
- `main` protegida: ninguém (nem agentes) faz push direto — sempre PR.

## Onde perguntar / mais contexto

- `CLAUDE.md` (raiz) — stack, regras do projeto, feature ativa
- `specs/001-phone-webcam-bridge/` — spec, plano, pesquisa técnica e tasks
  da feature em andamento
- Issues marcadas `good first issue` são um bom ponto de entrada
