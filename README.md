# CamLink

Transforme seu celular Android e câmeras IP em webcams virtuais — sem instalar
nada no celular.

O CamLink é um aplicativo desktop (Linux e Windows 10/11) que detecta
smartphones Android conectados por cabo USB e fontes de vídeo IP/RTSP, e os
expõe como webcams virtuais reconhecidas por OBS Studio, navegadores (Chrome,
Firefox — WebRTC), Discord e qualquer aplicativo que use câmera.

## Visão

- **Zero instalação no celular**: a comunicação com o Android acontece via
  USB/ADB; nenhum aplicativo é instalado no aparelho (Android 12+).
- **Controles de câmera em tempo real**: zoom, foco (tap-to-focus), exposição,
  balanço de branco, torch e estabilização, direto do desktop.
- **Modos inteligentes**: Auto, Night, Sport e Pro trocam parâmetros de
  câmera (foco, exposição, fps, estabilização, redução de ruído, AF por
  rosto) em runtime, sem interromper o stream; Pro libera controle manual
  completo.
- **Fontes IP/RTSP**: câmeras de rede como webcams virtuais, com credenciais
  guardadas apenas no cofre de segredos do sistema operacional.
- **Múltiplos dispositivos simultâneos**: cada fonte vira um dispositivo de
  webcam independente.
- **Captura RAW (DNG)**: fotos em RAW a partir do sensor do celular.
- **Paridade Linux + Windows**: funcionalidades equivalentes nas duas
  plataformas desde a v1.

## Status

🚧 **Ainda não há release publicada.** Todas as funcionalidades abaixo
existem e foram validadas em bancada, mas os instaladores ainda não estão
disponíveis para download — por enquanto é preciso
[compilar do código-fonte](#compilando-do-código-fonte). Acompanhe o
progresso em [`specs/001-phone-webcam-bridge/`](specs/001-phone-webcam-bridge/).

## Instalação

### Linux

O CamLink precisa do módulo `v4l2loopback`, que é o que cria o dispositivo de
webcam virtual, e de uma regra `udev` para que ele funcione **sem pedir sua
senha a cada fonte iniciada**. Os pacotes cuidam disso na instalação.

**Debian / Ubuntu / Zorin / Mint** — instale o `.deb` pelo gerenciador de
pacotes gráfico ou por linha de comando:

```bash
sudo apt install ./CamLink_0.1.0_amd64.deb
```

As dependências (`adb`, `ffmpeg`, `v4l-utils`, `v4l2loopback-dkms`) vêm da
distribuição. O `scrcpy` fica de fora de propósito: o que o Debian/Ubuntu
empacota é antigo demais (o CamLink precisa de ≥ 4.0) — veja
[scrcpy antigo demais](#scrcpy-antigo-demais).

**Arch / Manjaro / EndeavourOS** — pelo AUR:

```bash
yay -S camlink
```

**AppImage** — atenção, este é o único formato que **não se configura
sozinho**.

> ⚠️ **O AppImage exige um passo extra antes do primeiro uso.** Ele traz o
> aplicativo, mas a webcam virtual depende do `v4l2loopback` — um módulo do
> kernel e um utilitário de linha de comando que precisam vir da sua
> distribuição. Um AppImage não tem como instalar isso.
>
> **Se você pular este passo, o app abre normalmente e só falha na hora de
> iniciar a transmissão**, dizendo que o `v4l2loopback-ctl` não está
> instalado.

```bash
chmod +x CamLink_0.1.0_amd64.AppImage

# Passo obrigatório, uma única vez:
git clone --depth 1 https://github.com/Wolfloiz/CamLink.git
sudo ./CamLink/installer/linux/install.sh
```

O clone é necessário porque o script instala arquivos de configuração que
ficam ao lado dele.

**Se você prefere não fazer isso, use o `.deb` ou o pacote do AUR** — eles
declaram as dependências e configuram tudo na instalação, sem passo manual.

Depois de qualquer instalação, **faça logout e login** — seu usuário foi
adicionado ao grupo `video` e isso só vale na próxima sessão.

### Windows 10 e 11

Baixe o instalador (`CamLink_0.1.0_x64-setup.exe` ou o `.msi`) e execute. Ele
já traz tudo embutido — `adb`, `ffmpeg` e a câmera virtual — e registra a
câmera no sistema automaticamente. Não é preciso instalar driver de terceiros.

## Usando com o celular Android

Seu celular precisa de **Android 12 ou superior**. Nada é instalado nele.

**1. Habilite a depuração USB no celular** (uma vez só):

- Abra *Configurações → Sobre o telefone* e toque **7 vezes** em *Número da
  versão*. Aparece a mensagem "Você agora é um desenvolvedor".
- Volte para *Configurações → Sistema → Opções do desenvolvedor* e ligue
  **Depuração USB**.

**2. Conecte o cabo USB.** O celular aparece na lista do CamLink em até 3
segundos.

**3. Autorize no celular.** Na primeira conexão o Android mostra *"Permitir
depuração USB?"*. Marque **Sempre permitir deste computador** e toque em
**Permitir**. Enquanto isso não for feito, o CamLink lista o aparelho como
**Não autorizado** — veja
[a seção de solução de problemas](#o-celular-fica-não-autorizado).

**4. Clique em Iniciar transmissão.** A câmera virtual passa a existir e
aparece no OBS, no Chrome, no Firefox, no Discord e em qualquer programa que
use webcam, com o nome que você definir para a fonte.

> Use um cabo de **dados**. Muitos cabos que acompanham carregadores só
> conduzem energia, e nesses o celular nunca aparece na lista.

## Usando uma câmera IP (RTSP)

Na aba **Fontes RTSP**, dê um nome à fonte e informe a URL da câmera — por
exemplo `rtsp://192.168.0.42:554/stream`.

Se a câmera exigir login, há duas formas de informar as credenciais, e em
nenhuma delas a senha vai na URL:

- **Usuário na URL, senha no campo Senha** — escreva
  `rtsp://admin@192.168.0.42:554/stream` e preencha só a senha.
- **Tudo no campo Senha** — deixe a URL sem usuário e preencha o campo com
  `usuario:senha`.

A senha vai para o cofre de segredos do sistema operacional (Secret Service
no Linux, Gerenciador de Credenciais no Windows) e só é injetada na URL no
momento de conectar. Ela nunca é gravada no arquivo de configuração nem
aparece nos logs, que censuram a credencial antes de escrever.

## Solução de problemas

No Linux, o diagnóstico automático resolve a maior parte dos casos e **não
precisa de senha**:

```bash
/usr/share/camlink/install.sh --check
```

Ele lista o que está faltando, item por item:

```
CamLink — diagnóstico de pré-requisitos
  adb: /usr/bin/adb
  ffmpeg: /usr/bin/ffmpeg
  v4l2loopback-ctl: /usr/bin/v4l2loopback-ctl
  scrcpy: 4.0
  v4l2loopback-ctl: 0.15.4
  udev rule: /usr/lib/udev/rules.d/99-camlink-v4l2loopback.rules
  modules-load.d: /usr/lib/modules-load.d/camlink-v4l2loopback.conf
  módulo: carregado
  /dev/v4l2loopback: acessível por este usuário
Tudo pronto.
```

Itens em vermelho vêm com a instrução do que fazer. Quando algo falta, a
última linha diz quantos itens estão pendentes e manda rodar o instalador.

### O celular fica "Não autorizado"

O Android não recebeu (ou recusou) a autorização de depuração USB. Desconecte
e reconecte o cabo: o diálogo *"Permitir depuração USB?"* deve reaparecer.

Se ele não aparecer mais, é porque a chave deste computador foi memorizada com
"recusar". No celular, vá em *Opções do desenvolvedor → Revogar autorizações de
depuração USB*, reconecte e autorize.

### Nenhum celular aparece na lista

Nesta ordem: confirme que o cabo é de **dados** e não só de carga; que a
**Depuração USB** está ligada; e, no Linux, que o `adb` está instalado
(`install.sh --check` diz). Trocar a porta USB também resolve casos de hub com
pouca energia.

### "O utilitário v4l2loopback-ctl não está instalado"

Os pré-requisitos do sistema não foram instalados. Acontece tipicamente com o
**AppImage**, que não consegue instalá-los sozinho — veja a
[nota na seção de instalação](#linux).

```bash
sudo ./installer/linux/install.sh
```

Ou instale pela sua distribuição: o utilitário vem no pacote `v4l-utils`
(Debian/Ubuntu e Arch) e o módulo, no `v4l2loopback-dkms`.

Ele não é embutido no AppImage de propósito: precisa casar com o módulo do
kernel em uso, e uma versão descasada seria pior do que nenhuma.

### "Sem permissão para criar a câmera virtual"

Seu usuário não está no grupo `video`, ou está mas a sessão ainda não sabe
disso. **Faça logout e login.** Para conferir sem deslogar:

```bash
id -nG | tr ' ' '\n' | grep -x video
```

Se não imprimir nada, rode `sudo /usr/share/camlink/install.sh`.

### "O módulo v4l2loopback não está carregado"

```bash
sudo modprobe v4l2loopback
```

Se funcionar mas voltar a falhar depois de reiniciar, o arquivo que carrega o
módulo no boot não foi instalado — rode `sudo /usr/share/camlink/install.sh`.

### Secure Boot bloqueia o módulo

Em máquinas com Secure Boot ativo, o `v4l2loopback` só carrega se o módulo
estiver assinado — o erro aparece como *"Key was rejected by service"*. As
saídas são assinar o módulo com uma chave MOK própria ou desativar o Secure
Boot na UEFI. O CamLink detecta esse caso e mostra a orientação na própria
tela.

### scrcpy antigo demais

O CamLink precisa de **scrcpy ≥ 4.0** para fontes Android. Câmeras RTSP
funcionam sem ele. Debian, Ubuntu e derivados costumam empacotar versões bem
anteriores; para instalar a oficial:

```bash
sudo ./installer/linux/install.sh --with-scrcpy
```

No Arch a versão dos repositórios já serve.

### O celular aparece como incompatível

O CamLink exige **Android 12+**, porque usa uma API de câmera que não existe
antes disso. O motivo aparece ao lado do aparelho na lista.

### O Firefox não encontra a câmera (Linux)

Algumas versões do Firefox no Linux não enumeram dispositivos
`v4l2loopback`. O CamLink oferece abrir o Firefox com uma camada de
compatibilidade que resolve isso.

### A transmissão falha repetidamente num aparelho Samsung

Desconecte e reconecte o cabo USB, ou reinicie o aplicativo. É um problema
conhecido do scrcpy com a camada de câmera da Samsung, não do CamLink — os
detalhes e os relatos no upstream estão em
[Limitações conhecidas](#limitações-conhecidas).

### A câmera some do OBS ao girar ou trocar de câmera (Linux)

Comportamento conhecido, com explicação e contorno em
[Limitações conhecidas](#limitações-conhecidas).

### Só consigo uma captura RAW por vez

É proposital: um segundo pedido enquanto o primeiro roda é recusado com
`BUSY`. Espere o job atual terminar.

## Privacidade

O CamLink não envia nada para lugar nenhum. Não há telemetria, analytics,
verificação de atualização nem "phone home" de qualquer espécie. Isto é um
requisito do projeto (FR-026), não uma escolha de configuração — e foi
auditado, não apenas declarado:

- **Dependências**: das 274 crates na árvore de compilação, nenhuma é
  cliente HTTP, stack TLS ou SDK de telemetria. As únicas com capacidade de
  rede são `mio` e `socket2`, que são as primitivas de I/O do `tokio`.
- **Sockets do aplicativo**: existem exatamente dois no código, e ambos
  conectam em `127.0.0.1` — são os túneis que o `adb` abre para falar com o
  celular pelo cabo USB. Nenhum endereço externo é alcançável por eles.
- **Interface**: nenhum `fetch`, `XMLHttpRequest` ou `WebSocket`; nenhuma
  fonte, folha de estilo, script ou imagem vinda de CDN. Tudo que a tela
  carrega está dentro do pacote instalado.
- **Única saída de rede**: o `ffmpeg`, quando você configura uma fonte
  IP/RTSP, conecta no endereço que **você** digitou. É o propósito do
  recurso. Nenhum outro destino é contatado, e a saída do ffmpeg é sempre
  local (o dispositivo de câmera virtual ou a própria memória do app).
- **Credenciais**: senhas de câmeras RTSP ficam apenas no cofre do sistema
  operacional (Secret Service no Linux, Gerenciador de Credenciais no
  Windows). Nunca no arquivo de configuração, e os logs censuram a
  credencial na URL antes de escrever.
- **Vídeo**: os frames nunca saem da máquina. Vão do celular (USB) ou da
  câmera IP direto para o dispositivo de câmera virtual local, consumido
  por OBS/navegador/Discord na mesma máquina.

A política de segurança de conteúdo (CSP) da janela restringe o que a
interface pode carregar, de modo que nem um erro futuro nosso consiga
introduzir uma requisição externa sem que isso apareça como violação.

Os binários que o CamLink executa (`adb`, `scrcpy`, `ffmpeg`,
`v4l2loopback-ctl`) são de terceiros e vêm da sua distribuição ou embutidos
no instalador; o projeto não os modifica.

## Limitações conhecidas

- **Trocar de câmera (frontal/traseira), espelhar ou girar (qualquer ângulo)
  no Linux pode exigir atualizar a página no Meet/Chrome uma vez.** No
  Linux, o scrcpy escreve os frames direto no device v4l2loopback
  (`--v4l2-sink`) sem passar pelo CamLink — não há como aplicar espelho/giro
  "ao vivo" nesse caminho, então **toda** mudança de orientação reinicia o
  processo do scrcpy (não só 90°/270° como no Windows, onde o pipeline passa
  pelo Rust e permite atualização ao vivo — FR-016a). Isso gera um instante
  sem frame novo; o device virtual continua saudável durante esse instante
  (confirmado: nenhum evento de add/remove/change no kernel), mas o Meet às
  vezes marca a câmera como indisponível e não recupera sozinho — nem
  esperando alguns segundos. **F5 na aba (ou sair e entrar de novo na
  chamada) resolve.** Reproduzido também num Moto G55 (2026-08-03), então
  não é peculiaridade do S20 FE — parece ser inerente ao caminho de restart
  do scrcpy no Linux (`--v4l2-sink`), independente de fabricante.
- **Em alguns aparelhos Samsung, a transmissão pode falhar repetidamente
  depois de trocar de câmera ou girar a imagem.** O aparelho não libera a
  câmera a tempo e o CamLink não consegue reabri-la; depois de algumas
  tentativas ele desiste e pede para reconectar o cabo, em vez de ficar
  tentando indefinidamente. **Contorno**: desconecte e reconecte o cabo USB,
  ou reinicie o aplicativo.

  Não é um defeito do CamLink. É um problema conhecido e ainda em aberto do
  próprio scrcpy com a camada de câmera dos aparelhos Samsung, relatado em
  S22, SM-S906B e outros modelos:
  [#6514](https://github.com/Genymobile/scrcpy/issues/6514),
  [#5977](https://github.com/Genymobile/scrcpy/issues/5977),
  [#5311](https://github.com/Genymobile/scrcpy/issues/5311). Tentamos isolar
  o gatilho (resolução, sequência de giros, leitor de preview concorrente)
  sem conseguir reproduzir em testes controlados — o disparo parece exigir o
  padrão de uso completo do aplicativo. Enquanto não houver correção no
  upstream, não há o que o CamLink possa fazer além de falhar de forma
  previsível e avisar, que é o comportamento atual.

## Contribuindo

Veja [CONTRIBUTING.md](CONTRIBUTING.md) para o fluxo de PRs e issues.

### Stack

- **Backend**: Rust (Tauri 2.x) — `src-tauri/`
- **UI**: SvelteKit — `src/`
- **Android**: fork do [scrcpy](https://github.com/Genymobile/scrcpy)-server
  (Java 17, submodule em `scrcpy/`, branch `camlink`)
- **Runtime**: adb, scrcpy ≥ 4.0, ffmpeg, v4l2loopback ≥ 0.13 (Linux),
  filtro DirectShow próprio (Windows — sem driver de terceiros)

### Compilando do código-fonte

Pré-requisitos: `rustup`, `pnpm` e as bibliotecas de sistema do Tauri
(`webkit2gtk-4.1`, `gtk3`, `librsvg` no Linux).

```bash
git clone --recurse-submodules https://github.com/Wolfloiz/CamLink.git
cd CamLink
pnpm install
pnpm tauri dev          # roda em modo desenvolvimento
pnpm tauri build        # gera os instaladores da plataforma atual
```

O `pnpm tauri build` precisa dos binários de terceiros vendorizados antes:
`installer/linux/vendor.sh` (Linux) ou `installer/windows/vendor.ps1`
(Windows). Para apenas compilar ou rodar os testes, o modo stub basta e não
baixa nada:

```bash
./installer/linux/vendor.sh --stub
```

Gates de qualidade (obrigatórios, ver `.specify/memory/constitution.md`):

```bash
cd src-tauri
cargo fmt --check && cargo clippy -- -D warnings && cargo test
```

## Licença

[GPL-3.0](LICENSE). O fork do scrcpy-server permanece sob Apache-2.0 (licença
do upstream).
