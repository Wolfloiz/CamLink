# Feature Specification: CamLink — Câmeras Android e IP como webcams virtuais

**Feature Branch**: `001-phone-webcam-bridge`

**Created**: 2026-07-09

**Updated**: 2026-07-09 (reescrita a partir do documento de produto CamLink)

**Status**: Draft

**Input**: Documento de produto "CamLink — Especificação do Projeto": aplicativo
desktop que transforma smartphones Android (via cabo USB, sem instalar nada no
celular) e câmeras IP/RTSP em webcams virtuais reconhecidas por OBS Studio,
navegadores (WebRTC), Discord e qualquer aplicativo que use câmera; com controles
de câmera em tempo real, modos inteligentes de captura, captura RAW (DNG) e
múltiplos dispositivos simultâneos.

## Clarifications

### Session 2026-07-09

- Q: O documento CamLink propõe Linux na v1 e Windows como fase futura, mas a
  Constituição exige paridade Linux+Windows. Como resolver? → A: Manter a
  paridade — a v1 entrega Linux e Windows 10/11 com funcionalidades
  equivalentes.
- Q: O nome oficial do produto é "CamLink" (documento de produto) ou
  "DroidCamLink" (constituição)? → A: CamLink; a constituição será emendada.
- Q: Qual licença OSI o projeto adota (FR-025)? → A: GPL-3.0.
- Q: Qual o requisito mínimo de Android da v1? → A: Android 12+; aparelhos
  anteriores são listados como incompatíveis, com explicação.
- Q: Como armazenar credenciais de fontes RTSP entre sessões? → A: No cofre de
  segredos do sistema operacional; a configuração salva não expõe a senha.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Câmera Android via USB como webcam virtual (Priority: P1)

Um usuário conecta o celular Android ao computador com um cabo USB. O CamLink
detecta o aparelho automaticamente, sem exigir instalação de nenhum aplicativo no
celular. Com um clique, o vídeo da câmera (traseira ou frontal) passa a ser
transmitido para um dispositivo de webcam virtual do sistema, imediatamente
selecionável no OBS Studio, Chrome/Chromium, Firefox, Discord e qualquer outro
aplicativo que consuma câmeras.

**Why this priority**: É o núcleo do produto. Sem o caminho
"cabo conectado → webcam virtual funcionando", nenhuma outra capacidade tem valor.

**Independent Test**: Conectar um Android com depuração USB autorizada, iniciar a
transmissão no CamLink e selecionar o dispositivo virtual no OBS Studio e em uma
chamada de teste no navegador; o vídeo ao vivo deve aparecer em ambos, com atraso
imperceptível.

**Acceptance Scenarios**:

1. **Given** o CamLink aberto e um Android com depuração USB autorizada, **When**
   o usuário conecta o cabo USB, **Then** o aparelho aparece na lista de
   dispositivos detectados em até 3 segundos, sem qualquer instalação no celular.
2. **Given** um aparelho detectado, **When** o usuário inicia a transmissão,
   **Then** um dispositivo de webcam virtual é criado e o vídeo da câmera do
   celular fica disponível nele, com pré-visualização ao vivo no CamLink.
3. **Given** uma transmissão ativa, **When** o usuário seleciona o dispositivo
   virtual no OBS Studio, no Chrome (WebRTC), no Firefox ou no Discord, **Then**
   o vídeo é exibido corretamente em cada aplicativo, sem configuração adicional.
4. **Given** uma transmissão ativa em uso por um aplicativo, **When** o cabo USB
   é desconectado, **Then** o CamLink sinaliza a desconexão, o dispositivo
   virtual exibe imagem de espera (o aplicativo consumidor não trava) e a
   transmissão é retomada automaticamente ao reconectar o cabo.
5. **Given** um Android sem depuração USB autorizada, **When** o usuário conecta
   o cabo, **Then** o CamLink orienta passo a passo como habilitar e autorizar a
   depuração USB no aparelho.

---

### User Story 2 - Controles de câmera em tempo real (Priority: P2)

Durante a transmissão, o usuário ajusta a câmera do celular diretamente pelo
CamLink, sem interromper o stream: zoom digital contínuo, foco (automático
contínuo, toque-para-focar em uma região do preview, ou manual por distância),
compensação de exposição (-2 a +2 EV) ou exposição manual completa, ISO, balanço
de branco por modos pré-definidos, estabilização eletrônica (EIS) liga/desliga,
lanterna (torch) e troca entre câmera frontal e traseira.

**Why this priority**: É o principal diferencial declarado do produto frente a
alternativas que só transmitem vídeo bruto.

**Independent Test**: Com transmissão ativa consumida pelo OBS, acionar cada
controle (zoom, tap-to-focus, EV, ISO, balanço de branco, EIS, torch) e verificar
que o efeito aparece no vídeo entregue sem queda ou reinício do stream; alternar
frontal/traseira e confirmar retomada em até 2 segundos.

**Acceptance Scenarios**:

1. **Given** uma transmissão ativa, **When** o usuário ajusta zoom, exposição,
   ISO ou balanço de branco, **Then** o efeito é visível no vídeo entregue aos
   aplicativos em menos de 1 segundo, sem interrupção do stream.
2. **Given** uma transmissão ativa, **When** o usuário toca em uma região do
   preview, **Then** a câmera foca naquela região (tap-to-focus).
3. **Given** uma transmissão ativa, **When** o usuário alterna entre câmera
   frontal e traseira, **Then** o vídeo retorna em no máximo 2 segundos e o
   aplicativo consumidor continua funcionando sem reselecionar o dispositivo.
4. **Given** uma transmissão ativa, **When** o usuário aciona a lanterna,
   **Then** a lanterna do celular acende/apaga imediatamente.
5. **Given** um aparelho que não suporta determinado controle (ex.: ISO manual),
   **When** o painel de controles é exibido, **Then** o controle não suportado
   aparece desabilitado ou oculto, com indicação da limitação.

---

### User Story 3 - Modos inteligentes de captura (Priority: P2)

O usuário escolhe um dos quatro modos pré-configurados que otimizam os parâmetros
da câmera automaticamente: **Auto** (foco contínuo, exposição automática,
estabilização ativa — videochamadas), **Night** (exposições mais longas a
15-30 fps, redução de ruído de alta qualidade, +1 EV, foco automático em rosto —
pouca luz), **Sport** (60 fps fixos, estabilização desligada, redução de ruído
rápida — ação), **Pro** (controle manual completo de foco, exposição, ISO e
balanço de branco).

**Why this priority**: Entrega o valor dos controles avançados para usuários que
não querem ajustar parâmetro por parâmetro; diferencial competitivo declarado.

**Independent Test**: Com transmissão ativa, alternar entre os quatro modos e
verificar que os parâmetros correspondentes são aplicados (ex.: Sport trava
60 fps; Night aplica +1 EV) sem interromper o stream; no modo Pro, verificar que
todos os controles manuais ficam liberados.

**Acceptance Scenarios**:

1. **Given** uma transmissão ativa, **When** o usuário seleciona um modo,
   **Then** os parâmetros do modo são aplicados sem interromper o stream e o modo
   ativo fica indicado na interface.
2. **Given** o modo Sport ativo, **When** o vídeo é consumido por um aplicativo,
   **Then** a transmissão mantém 60 fps (quando o aparelho suporta).
3. **Given** o modo Pro ativo, **When** o usuário ajusta manualmente foco,
   exposição, ISO e balanço de branco, **Then** todos respondem livremente sem
   interferência de automatismos.

---

### User Story 4 - Câmeras IP/RTSP como webcams virtuais (Priority: P3)

O usuário adiciona uma câmera IP ou stream RTSP (ex.: câmera de segurança,
dashcam IP) informando a URL — com autenticação embutida na URL quando necessária
— e o CamLink a expõe como um dispositivo de webcam virtual independente,
utilizável em qualquer aplicativo, com baixa latência.

**Why this priority**: Amplia o produto para fontes além do Android, mas o valor
principal (celular como webcam) não depende disso.

**Independent Test**: Adicionar uma URL RTSP válida com autenticação, iniciar a
transmissão e selecionar o novo dispositivo virtual no OBS; o vídeo da câmera IP
deve aparecer com atraso máximo de 300 ms.

**Acceptance Scenarios**:

1. **Given** uma URL RTSP válida, **When** o usuário a adiciona e inicia a
   fonte, **Then** um dispositivo virtual próprio é criado e o vídeo fica
   disponível aos aplicativos com atraso de até 300 ms.
2. **Given** uma URL com credenciais incorretas, **When** a conexão falha,
   **Then** o CamLink exibe erro claro de autenticação sem travar a interface.
3. **Given** uma fonte RTSP ativa, **When** o stream de origem cai, **Then** o
   dispositivo virtual exibe imagem de espera e a reconexão é tentada
   automaticamente.

---

### User Story 5 - Captura RAW (DNG) (Priority: P3)

Fotógrafos e videomakers capturam a qualidade máxima do sensor do celular:
**Snapshot RAW** (um único quadro DNG na resolução nativa do sensor, para
calibração de cor ou referência) e **Sequência RAW** (captura contínua em 1-3 fps,
com cadência calculada dinamicamente conforme a banda USB disponível, para
pós-produção). Os arquivos DNG são salvos no computador e abrem em editores RAW
consagrados (RawTherapee, Darktable e similares). Se o aparelho não suportar RAW,
os controles correspondentes são ocultados.

**Why this priority**: Atende um público específico (pós-produção); o produto é
completo sem isso.

**Independent Test**: Em um aparelho com suporte RAW, capturar um snapshot DNG e
uma sequência de 10 segundos; abrir os arquivos no RawTherapee ou Darktable e
verificar integridade e resolução nativa do sensor. Em um aparelho sem RAW,
verificar que os controles não aparecem.

**Acceptance Scenarios**:

1. **Given** um aparelho com suporte RAW conectado, **When** o usuário aciona
   Snapshot RAW, **Then** um arquivo DNG na resolução nativa do sensor é salvo no
   computador e abre corretamente em editores RAW compatíveis.
2. **Given** uma Sequência RAW em andamento, **When** a banda USB disponível
   varia, **Then** a cadência de captura se ajusta dinamicamente entre 1 e 3 fps
   sem corromper arquivos.
3. **Given** um aparelho sem suporte RAW, **When** ele é selecionado, **Then**
   os controles de RAW ficam ocultos.

---

### User Story 6 - Múltiplas fontes simultâneas (Priority: P3)

O usuário usa mais de uma fonte ao mesmo tempo — por exemplo, a câmera traseira
do Android como webcam principal e uma câmera IP como segunda fonte — cada uma
exposta em seu próprio dispositivo de webcam virtual, selecionáveis
independentemente nos aplicativos.

**Why this priority**: Cenário avançado (multi-câmera em streaming); valioso, mas
posterior ao fluxo de fonte única.

**Independent Test**: Ativar simultaneamente um Android via USB e uma fonte RTSP;
no OBS, adicionar as duas como fontes de câmera distintas e verificar vídeo
fluido e independente em ambas.

**Acceptance Scenarios**:

1. **Given** um Android e uma câmera IP configurados, **When** ambos são
   ativados, **Then** cada fonte tem seu próprio dispositivo virtual e ambas
   transmitem simultaneamente.
2. **Given** duas fontes ativas, **When** uma delas cai, **Then** a outra
   continua transmitindo sem interferência.

---

### Edge Cases

- Celular conectado com depuração USB desabilitada ou não autorizada: o CamLink
  detecta o estado e guia o usuário pela habilitação (sem isso não há fonte).
- Cabo USB desconectado durante transmissão em uso: dispositivo virtual entra em
  imagem de espera; aplicativos consumidores não travam; reconexão automática ao
  religar o cabo.
- Tela do celular bloqueada ou celular reiniciado durante o uso: transmissão
  sinalizada como interrompida e retomada quando o aparelho voltar.
- Aparelho sem suporte a determinados recursos (RAW, ISO manual, EIS, 60 fps):
  controles correspondentes ocultos/desabilitados com indicação clara — nunca
  falha silenciosa.
- Múltiplos aparelhos Android conectados ao mesmo tempo: lista distingue os
  aparelhos por nome/modelo e o usuário escolhe qual(is) ativar.
- Banda USB insuficiente para a configuração escolhida (ex.: resolução alta +
  sequência RAW): o CamLink reduz a cadência RAW dinamicamente e informa o
  usuário; o stream principal tem prioridade.
- URL RTSP inválida, host inacessível ou credenciais erradas: mensagens de erro
  específicas e acionáveis.
- Aplicativo consumidor abre antes de qualquer fonte ativa: o dispositivo
  virtual, quando existente, exibe imagem de espera.
- Encerramento do CamLink com transmissão ativa: aplicativos consumidores não
  travam; dispositivos virtuais são removidos ou entram em espera.
- Instalação em distribuição sem os pré-requisitos de sistema: o instalador
  configura dependências, regras de acesso a dispositivos e persistência
  necessárias, sem exigir uso de terminal do usuário final.

## Requirements *(mandatory)*

### Functional Requirements

**Fonte Android (USB)**

- **FR-001**: O sistema MUST detectar automaticamente aparelhos Android
  conectados por cabo USB (com depuração USB autorizada) e listá-los em até 3
  segundos, sem exigir instalação de qualquer aplicativo no celular.
- **FR-002**: O sistema MUST orientar o usuário, passo a passo, quando a
  depuração USB estiver desabilitada ou não autorizada.
- **FR-002a**: O requisito mínimo da fonte Android é Android 12; aparelhos com
  versões anteriores MUST ser listados como incompatíveis, com explicação clara
  do motivo.
- **FR-003**: O sistema MUST transmitir o vídeo da câmera selecionada (traseira
  ou frontal) do aparelho para um dispositivo de webcam virtual do sistema
  operacional.
- **FR-004**: O atraso da fonte Android MUST ser de no máximo 70 ms em condições
  normais (tipicamente 35–70 ms — comparável ou melhor que webcams USB físicas),
  nas duas plataformas.

**Webcam virtual e compatibilidade**

- **FR-005**: O dispositivo de webcam virtual MUST ser reconhecido
  automaticamente por OBS Studio, Chrome/Chromium (WebRTC), Firefox (WebRTC),
  Discord e qualquer aplicativo que consuma câmeras padrão do sistema, tanto no
  Linux quanto no Windows 10/11, com paridade de funcionalidades.
- **FR-006**: Em perda de fonte (cabo desconectado, stream caído), o dispositivo
  virtual MUST exibir imagem de espera em vez de congelar ou travar os
  aplicativos consumidores, e a transmissão MUST ser retomada automaticamente
  quando a fonte voltar.
- **FR-007**: O usuário MUST poder configurar resolução, taxa de quadros,
  bitrate e codec de vídeo (H.264/H.265) por fonte.

**Controles de câmera em tempo real** (aplicados sem interromper o stream)

- **FR-008**: Zoom digital contínuo.
- **FR-009**: Foco em três formas: automático contínuo, toque-para-focar em
  região do preview, e manual por distância.
- **FR-010**: Exposição: compensação de -2 a +2 EV ou modo manual completo.
- **FR-011**: ISO controlável em modo manual.
- **FR-012**: Balanço de branco por modos pré-definidos (luz do dia, nublado,
  fluorescente, etc.).
- **FR-013**: Estabilização eletrônica de vídeo (EIS) ativável/desativável.
- **FR-014**: Lanterna do celular (torch) acionável pelo desktop.
- **FR-015**: Troca entre câmera frontal e traseira com interrupção máxima de 2
  segundos, sem reconfiguração do aplicativo consumidor.
- **FR-016**: Controles não suportados pelo aparelho MUST aparecer
  desabilitados/ocultos com indicação da limitação.

**Modos inteligentes**

- **FR-017**: O sistema MUST oferecer quatro modos pré-configurados — Auto
  (foco contínuo, exposição automática, EIS ativa), Night (exposições longas a
  15-30 fps, redução de ruído de alta qualidade, +1 EV, foco em rosto), Sport
  (60 fps fixos, EIS desligada, redução de ruído rápida) e Pro (controle manual
  completo) — aplicáveis durante a transmissão sem interrompê-la.

**Fontes IP/RTSP**

- **FR-018**: O sistema MUST aceitar fontes de câmera IP/RTSP por URL, com
  autenticação embutida na URL, expondo cada uma como dispositivo virtual
  independente com atraso máximo de 300 ms.
- **FR-018a**: Credenciais de fontes RTSP lembradas entre sessões MUST ser
  armazenadas no cofre de segredos do sistema operacional; nenhuma senha MUST
  ficar legível em arquivos de configuração.

**Captura RAW**

- **FR-019**: Em aparelhos compatíveis, o sistema MUST capturar Snapshot RAW
  (um quadro DNG na resolução nativa do sensor) e Sequência RAW (1-3 fps, com
  cadência ajustada dinamicamente à banda USB disponível), salvando os arquivos
  no computador em formato DNG compatível com editores RAW (RawTherapee,
  Darktable e similares).
- **FR-020**: Durante a Sequência RAW, o stream de vídeo principal MUST manter
  prioridade sobre a captura RAW na disputa por banda.

**Multi-dispositivo**

- **FR-021**: O sistema MUST suportar múltiplas fontes simultâneas (Android USB
  e/ou IP/RTSP), cada uma em seu próprio dispositivo virtual, com falha de uma
  fonte isolada das demais.

**Interface e experiência**

- **FR-022**: A interface MUST apresentar: lista de dispositivos detectados
  (Android USB + RTSP), preview ao vivo, seletor de modo inteligente, painel de
  controles (sliders, toggles, tap-to-focus no preview), configurações de
  resolução/FPS/bitrate/codec, indicadores de status, capacidades RAW e o
  identificador do dispositivo virtual em uso.
- **FR-023**: O preview ao vivo MUST ser leve (na ordem de 1 quadro por segundo)
  e não degradar o stream principal consumido pelos aplicativos.

**Distribuição e princípios**

- **FR-024**: O produto MUST ser distribuído para Linux em pacote Debian/Ubuntu
  (.deb), AppImage (genérico) e AUR (Arch), e para Windows 10/11 em instalador
  gráfico; em ambos, a instalação configura dependências de sistema, permissões
  de acesso a dispositivos e persistência necessárias, sem exigir uso de
  terminal do usuário final.
- **FR-025**: O projeto MUST ser open source, com código público sob a licença
  GPL-3.0.
- **FR-026**: Nenhum vídeo ou dado pessoal MUST sair do computador; a única
  comunicação de rede é o consumo de fontes RTSP configuradas pelo usuário.

### Out of Scope (v1)

- Gravação de vídeo pelo próprio CamLink (usar OBS/ffmpeg sobre o dispositivo
  virtual).
- Transmissão do celular por rede (Wi-Fi) — a fonte Android é exclusivamente via
  cabo USB.
- Aplicativo instalado no celular (é premissa de produto não existir).
- Captura/encaminhamento de áudio do celular.

### Key Entities

- **Fonte Android**: aparelho conectado via USB; atributos: nome/modelo, estado
  de autorização, câmeras disponíveis, capacidades (RAW, EIS, ISO manual,
  60 fps).
- **Fonte IP/RTSP**: stream remoto configurado por URL; atributos: URL, estado,
  latência; credenciais persistidas apenas no cofre de segredos do sistema.
- **Dispositivo virtual**: a webcam exposta ao sistema operacional; um por fonte
  ativa; atributos: identificador, formato ativo, estado (transmitindo/espera).
- **Sessão de transmissão**: vínculo fonte → dispositivo virtual; atributos:
  resolução, fps, bitrate, codec, estatísticas.
- **Estado de controles**: valores atuais de zoom, foco, exposição, ISO, balanço
  de branco, EIS, torch para uma sessão.
- **Modo inteligente**: perfil pré-configurado (Auto, Night, Sport, Pro) que
  define o estado de controles.
- **Captura RAW**: trabalho de captura (snapshot ou sequência) com destino em
  disco e cadência efetiva.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Da conexão do cabo USB (aparelho já autorizado) até o vídeo
  disponível em um aplicativo consumidor: no máximo 30 segundos, sem tocar no
  celular e sem instalar nada nele.
- **SC-002**: Atraso fim-a-fim da fonte Android de no máximo 70 ms (tipicamente
  35–70 ms), no Linux e no Windows; de fontes RTSP, no máximo 300 ms.
- **SC-003**: O dispositivo virtual é reconhecido e utilizável em OBS Studio,
  Chrome, Firefox e Discord sem qualquer configuração manual além de
  selecioná-lo.
- **SC-004**: Todo controle de câmera aplicado reflete no vídeo entregue em
  menos de 1 segundo, sem queda do stream; a troca frontal/traseira interrompe o
  vídeo por no máximo 2 segundos.
- **SC-005**: Transmissão sustentada por sessões de pelo menos 2 horas sem
  interrupção perceptível ou vazamento de recursos.
- **SC-006**: Arquivos DNG capturados abrem sem erro nos editores RAW de
  referência (RawTherapee, Darktable) e correspondem à resolução nativa do
  sensor.
- **SC-007**: Duas fontes simultâneas transmitem de forma independente, e a
  queda de uma não afeta a outra.
- **SC-008**: Um usuário final instala o CamLink pelos pacotes oficiais e chega
  ao primeiro vídeo funcionando sem abrir um terminal.
- **SC-009**: 90% dos usuários de teste concluem a primeira transmissão sem
  consultar documentação externa.
- **SC-010**: O fluxo completo (detectar, transmitir, controlar, usar no OBS)
  funciona com resultados equivalentes no Linux e no Windows 10/11.

## Assumptions

- **Mecanismo de conexão Android**: cabo USB com o mecanismo de depuração do
  Android (mesmo usado pelo scrcpy), que dispensa aplicativo no celular; o
  usuário precisa habilitar e autorizar a depuração USB uma única vez por
  computador — o produto orienta esse passo.
- **Dependência declarada**: o pipeline de vídeo do scrcpy é usado como cliente/
  dependência (premissa de produto do documento de origem; detalhes ficam para o
  plano).
- **Plataforma**: a v1 entrega Linux e Windows 10/11 com paridade de
  funcionalidades, conforme o Princípio IV da constituição (decisão registrada
  em Clarifications). O mecanismo de câmera virtual é o nativo de cada sistema.
- **Nome do produto**: CamLink (formerly referred to as "DroidCamLink";
  decisão registrada em Clarifications — emenda constitucional pendente).
- **Modelos suportados**: Android 12 ou superior (requisito firmado em
  Clarifications); aparelhos incompatíveis são listados com a limitação
  indicada.
- **Efeitos criativos** (filtros de cor, desfoque) da especificação anterior
  foram substituídos pelos controles reais de câmera e modos inteligentes do
  documento de produto; não fazem parte desta versão.
- **Presets nomeados pelo usuário** não constam do documento de produto; os
  quatro modos inteligentes cumprem esse papel nesta versão.
- **Privacidade**: sem conta, sem nuvem, sem telemetria.
- **Limite de fontes simultâneas**: limite prático de 4 fontes ativas ao mesmo
  tempo na v1 (decisão registrada no plano; FR-021).
