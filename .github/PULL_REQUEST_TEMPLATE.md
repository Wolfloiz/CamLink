## O que muda

<!-- resumo curto, 1-3 frases -->

## Por quê

<!-- motivação — issue relacionada, bug encontrado, etc. Use "Closes #123" se fechar uma issue -->

## Como testar

<!-- passos manuais, se aplicável (bancada Linux/Windows) -->

## Checklist

- [ ] `cargo fmt --check` / `cargo clippy -- -D warnings` / `cargo test` passam localmente
- [ ] Testado em pelo menos uma plataforma (Linux ou Windows) — qual?
- [ ] `specs/001-phone-webcam-bridge/tasks.md` atualizado, se a mudança fecha ou muda uma task
- [ ] Sem `unwrap()` em caminho falível; `unsafe` (se houver) tem comentário `// SAFETY:`
