# Changelog

Formato baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/);
o projeto segue [Versionamento Semântico](https://semver.org/lang/pt-BR/).

## [Não publicado]

### 0.1.0 — primeira versão

Ainda **não publicada**: o código está completo e validado em bancada, mas a
validação de instalação limpa (Ubuntu, Arch, Windows 10/11) e o teste de
estabilidade de 2 h ainda não foram concluídos. Acompanhe em
[`specs/001-phone-webcam-bridge/tasks.md`](specs/001-phone-webcam-bridge/tasks.md).

#### Adicionado

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
- **Câmera virtual própria no Windows**, via um filtro DirectShow do próprio
  CamLink — sem depender de driver de terceiros.
- **Diagnóstico de pré-requisitos** no Linux: `install.sh --check` lista o
  que falta, item por item, sem precisar de senha.

#### Segurança e privacidade

- Nenhuma telemetria, analytics ou verificação de atualização. A única
  comunicação de rede é o consumo das fontes RTSP configuradas pelo usuário;
  auditado e descrito em [README.md](README.md#privacidade).
- Senhas de câmeras RTSP ficam apenas no cofre de segredos do sistema
  operacional, nunca no arquivo de configuração, e são censuradas nos logs.
- A janela do aplicativo tem política de segurança de conteúdo (CSP)
  restritiva, impedindo que a interface carregue qualquer recurso externo.
- O aplicativo roda sem privilégios: o acesso ao dispositivo de vídeo virtual
  vem do grupo `video`, não de `sudo` ou `pkexec`.

#### Limitações conhecidas

Descritas em [README.md](README.md#limitações-conhecidas). Em resumo: no
Linux, girar ou trocar de câmera pode exigir recarregar a página no
Meet/Chrome; alguns aparelhos Samsung podem entrar em ciclo de reconexão por
um bug conhecido do próprio scrcpy; e o preview interno pode piscar com
fontes Android no Linux quando há um consumidor ativo (não afeta a imagem
entregue ao OBS).
