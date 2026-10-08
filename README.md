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

> 🚧 **Os pacotes abaixo ainda não estão publicados.** Enquanto não houver
> release, os instaladores precisam ser
> [compilados do código-fonte](#compilando-do-código-fonte). As instruções
> desta seção descrevem como será a instalação quando os arquivos estiverem
> disponíveis.

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
distribuição. O `scrcpy` **não** é uma delas: o cliente vai embutido no
pacote, na versão exata que o CamLink precisa — você não instala nem
atualiza nada por fora. Veja [por que embutido](#por-que-o-scrcpy-vai-embutido).

**Arch / Manjaro / EndeavourOS** — o pacote **ainda não está no AUR**, então
`yay -S camlink` não encontra nada. O `PKGBUILD` existe em
`installer/linux/PKGBUILD` e passa a funcionar assim que houver uma release
publicada, porque ele baixa os fontes a partir da tag.

Por enquanto, no Arch, use o AppImage ou compile do código-fonte.

**AppImage** — baixe o `.AppImage` e o `install-appimage.sh` da mesma
release, e rode:

```bash
bash install-appimage.sh
```

Ele encontra o AppImage na pasta de downloads, instala em `~/.local/bin`,
cria a entrada no menu de aplicativos com o ícone, e — só se faltar algo —
pede sudo uma única vez para instalar o `v4l2loopback`. Para remover:
`bash install-appimage.sh --uninstall`.

> **Por que um script, se AppImage não precisa de instalação?** Porque sem
> ele o arquivo fica inerte: o bit de execução não sobrevive ao download, e
> gerenciadores de arquivos modernos não executam binários por duplo clique
> (o Nautilus removeu isso por segurança). O sistema então diz *"não há
> aplicativo instalado para AppImage"* — mensagem que sugere procurar um
> programa, quando o problema é outro. O script também resolve o
> `v4l2loopback`, que um AppImage não tem como instalar sozinho.

Se preferir não usar o script, dá para rodar direto pelo terminal, mas o app
não aparecerá no menu:

```bash
chmod +x CamLink_0.1.0_amd64.AppImage
./CamLink_0.1.0_amd64.AppImage
```

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
  scrcpy embutido: scrcpy 4.1
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

Ou instale pela sua distribuição: o utilitário vem no pacote
`v4l2loopback-utils` e o módulo, no `v4l2loopback-dkms`. Note que **não** é o
`v4l-utils` — esse é outro projeto, que traz o `v4l2-ctl` e afins.

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

### Por que o scrcpy vai embutido

O CamLink usa um fork do `scrcpy-server` (a parte que roda no celular) e o
scrcpy **recusa funcionar se cliente e servidor não forem exatamente da
mesma versão** — ele aborta com `The server version (X) does not match the
client (Y)`. Não é um piso de versão: é igualdade.

Por isso o cliente não vem da distribuição. Até a versão 0.1.0 ele vinha, e
o resultado era previsível: o Arch atualizou o pacote para 4.1 enquanto o
fork estava na 4.0 e o CamLink parou de conectar em qualquer celular, numa
máquina onde nada do CamLink havia mudado. Agora o cliente é empacotado
junto, e cada release do CamLink carrega o par cliente+servidor casado.

Se você tiver um `scrcpy` instalado pela distro, ele é ignorado e continua
funcionando normalmente para o seu próprio uso. Para confirmar qual o
CamLink está usando:

```bash
./installer/linux/install.sh --check   # linha "scrcpy embutido:"
```

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
no instalador; o projeto não os modifica. O `scrcpy` embutido é o build
oficial do upstream, baixado da release do Genymobile sem alteração — só o
`scrcpy-server`, que roda no celular, é o nosso fork (código em `scrcpy/`).

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

  **Medido em sessão longa** (SM-S921B, Android 16, 2 h 18 min, 2026-10-08):
  17 desconexões de câmera, das quais o aplicativo se recuperou em todas.
  O padrão não é degradação progressiva — houve um trecho de 38 minutos sem
  nenhuma falha entre duas rajadas. As mensagens vêm do servidor no celular
  (`Camera disconnected`, `Camera capture failed: frame N`), e são os
  callbacks `onDisconnected`/`onCaptureFailed` do Android emitidos por código
  do scrcpy upstream, não do fork do CamLink — confirmado por `git blame`.
  Na mesma sessão o consumo de memória CAIU (560 → 358 MiB), então não há
  vazamento associado; o que o quirk causa é interrupção, não degradação.

## Contribuindo

Veja [CONTRIBUTING.md](CONTRIBUTING.md) para o fluxo de PRs e issues.

### Stack

- **Backend**: Rust (Tauri 2.x) — `src-tauri/`
- **UI**: SvelteKit — `src/`
- **Android**: fork do [scrcpy](https://github.com/Genymobile/scrcpy)-server
  (Java 17, submodule em `scrcpy/`, branch `camlink-4.1` — rebaseada sobre a
  tag `v4.1` do upstream, que é a versão do cliente embutido)
- **Runtime**: adb, ffmpeg, v4l2loopback ≥ 0.13 (Linux), scrcpy embutido,
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

O `pnpm tauri dev` precisa do jar do fork (`scrcpy-server-camlink`), que é o
servidor enviado ao celular. Em dev ele não vem de pacote nenhum, então
aponte a variável para ele:

```bash
export SCRCPY_SERVER_PATH="$PWD/scrcpy/dist/scrcpy-server-camlink"
pnpm tauri dev
```

Sem isso, iniciar uma fonte Android falha com `scrcpy_server_jar_ausente` e a
dica de como resolver. O `scrcpy-server` oficial da sua distribuição **não**
serve no lugar dele: ele não traz o servidor de controle do CamLink, e usá-lo
daria vídeo com os controles mudos.

Há dois jeitos de obter o jar. Construir exige JDK 17 e o Android SDK:

```bash
cd scrcpy && ./build-camlink.sh dist && cd ..
```

Ou baixar o da release, que é o mesmo artefato e já vem na versão certa:

```bash
gh release download v0.1.0 --repo Wolfloiz/CamLink \
  --pattern 'scrcpy-server-camlink' --dir scrcpy/dist
```

Para gerar pacote, depois disso:

```bash
./installer/linux/vendor.sh          # adb, ffmpeg, cliente scrcpy e o jar
```

O `vendor.sh` recusa empacotar se o jar e o cliente não forem da mesma
versão, porque esse par divergente é justamente o que impede qualquer
celular de conectar.

Gates de qualidade (obrigatórios, ver `.specify/memory/constitution.md`):

```bash
cd src-tauri
cargo fmt --check && cargo clippy -- -D warnings && cargo test
```

## Licença

[GPL-3.0](LICENSE). O fork do scrcpy-server permanece sob Apache-2.0 (licença
do upstream).
