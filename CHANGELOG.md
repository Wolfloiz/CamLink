# Changelog

Formato baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/);
o projeto segue [Versionamento Semântico](https://semver.org/lang/pt-BR/).

## [0.1.0] — 2026-11-10

Primeira versão. Validada em instalação limpa no Ubuntu 22.04/24.04, Arch e
Windows 10/11, com sessão contínua de 2 h no Linux sem vazamento de memória.

### Adicionado

- **Celular Android como webcam virtual, sem instalar nada no aparelho.**
  A comunicação acontece por USB/ADB com um fork do `scrcpy-server` enviado
  ao celular em tempo de execução (Android 12+).
- **Controles de câmera em tempo real**: zoom, foco com toque no preview,
  exposição, ISO, balanço de branco, lanterna e estabilização, sem
  interromper a transmissão.
- **Modos inteligentes** Auto, Night, Sport e Pro, que trocam conjuntos de
  parâmetros em runtime; Pro libera o controle manual completo.
- **Girar e espelhar** a imagem durante a transmissão.
- **Fontes IP/RTSP** como webcams virtuais, com reconexão automática e
  imagem de espera quando a fonte cai.
- **Até quatro fontes simultâneas**, misturando celulares e câmeras IP, cada
  uma como um dispositivo de webcam independente. A falha de uma não afeta
  as outras.
- **Captura RAW (DNG)** a partir do sensor do celular.
- **Seletor de tema** Claro / Escuro / Sistema, com a escolha lembrada.
- **Instaladores**: `.deb` e AppImage no Linux, `PKGBUILD` para o AUR, NSIS e
  MSI no Windows. Os pacotes configuram o `v4l2loopback` e o grupo `video`
  na instalação, sem exigir terminal.
- **O cliente `scrcpy` vem embutido** nos pacotes Linux, na versão exata
  exigida pelo fork que roda no celular. Você não instala nem atualiza nada
  por fora, e uma atualização do `scrcpy` da sua distribuição não quebra o
  CamLink — [por quê](README.md#por-que-o-scrcpy-vai-embutido).
- **Câmera virtual própria no Windows**, via um filtro DirectShow do próprio
  CamLink — sem depender de driver de terceiros.
- **Diagnóstico de pré-requisitos** no Linux: `install.sh --check` lista o
  que falta, item por item, sem precisar de senha.

### Segurança e privacidade

- A senha de uma câmera RTSP não aparece mais no log nem na tela quando o
  `ffmpeg` a ecoa numa mensagem de erro.
- Nenhuma telemetria, analytics ou verificação de atualização. A única
  comunicação de rede é o consumo das fontes RTSP configuradas pelo usuário;
  auditado e descrito em [README.md](README.md#privacidade).
- Senhas de câmeras RTSP ficam apenas no cofre de segredos do sistema
  operacional, nunca no arquivo de configuração, e são censuradas nos logs.
- A janela do aplicativo tem política de segurança de conteúdo (CSP)
  restritiva, impedindo que a interface carregue qualquer recurso externo.
- O aplicativo roda sem privilégios: o acesso ao dispositivo de vídeo virtual
  vem do grupo `video`, não de `sudo` ou `pkexec`.

### Corrigido antes do lançamento

- Uma resposta 401 de uma fonte RTSP sem senha deixou de mandar o usuário
  procurar uma senha que não existe, e o log passou a mostrar o motivo da
  recusa.

### Limitações conhecidas

Descritas em [README.md](README.md#limitações-conhecidas). Em resumo: no
Linux, girar ou trocar de câmera pode exigir recarregar a página no
Meet/Chrome; alguns aparelhos Samsung podem entrar em ciclo de reconexão por
um bug conhecido do próprio scrcpy; e o preview interno pode piscar com
fontes Android no Linux quando há um consumidor ativo (não afeta a imagem
entregue ao OBS).

[0.1.0]: https://github.com/Wolfloiz/CamLink/releases/tag/v0.1.0
