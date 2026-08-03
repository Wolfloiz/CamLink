# CamLink — Brief funcional para design de interface

Documento de referência com **todas as funções do CamLink** (implementadas e
planejadas), pra quem for desenhar a interface. Fonte: `spec.md`,
`data-model.md`, `contracts/`, e o estado real do código (`src-tauri/src/`,
`src/lib/`) em 2026-07-31. Onde a UI atual (funcional, mas sem design
dedicado) já resolve a função, isso está indicado — não é pra redesenhar do
zero, é material de referência pra dar forma visual ao que já existe e prever
espaço pro que ainda falta implementar.

## 1. O que é o produto

App desktop (Linux + Windows 10/11) que transforma câmeras Android — via
cabo USB, sem instalar nada no celular — e câmeras IP/RTSP em webcams
virtuais reconhecidas por OBS Studio, Chrome, Firefox e Discord. Diferencial:
controles reais de câmera em tempo real e modos inteligentes de captura,
aplicados sem interromper o stream. Sem conta, sem nuvem, sem telemetria —
nada sai do computador exceto o consumo das URLs RTSP que o próprio usuário
configura.

**Janela única, sem multi-view** hoje (`src/routes/+page.svelte`) — layout
vertical simples de seções empilhadas. Não há requisito de múltiplas janelas
ou telas separadas; a interface pode reorganizar essas seções livremente
(abas, colunas, cards), mas todo o inventário abaixo precisa caber num fluxo
coeso.

## 2. Princípios que a interface precisa respeitar

- **Nunca esconder falha atrás de silêncio** (FR-016): controle não suportado
  pelo aparelho aparece desabilitado/oculto **com explicação visível** do
  motivo, nunca some sem dizer por quê.
- **Toda mudança de controle é otimista + confirmada por evento**: o comando
  vai pro celular, e o estado real volta por um evento (`control_state`) que
  pode divergir do que foi pedido (outro cliente pode ter mudado por fora) —
  a UI reflete o estado confirmado, não assume que o pedido teve efeito.
- **Latência de efeito é um requisito de produto, não só técnico**: a maioria
  dos controles reflete no vídeo em <1s; troca de câmera e rotação 90°/270°
  têm orçamento de até 2s de interrupção — a UI deve comunicar esse tempo de
  espera (spinner/estado "busy"), não parecer travada.
- **Erros têm sempre uma ação sugerida** (`AppError.action_hint`) — a UI
  sempre tem espaço pra mostrar não só o erro, mas o que fazer a respeito.
- **Paridade Linux/Windows com pequenas divergências de comportamento**
  documentadas na seção 13 — a interface é a mesma, mas alguns tooltips/avisos
  mudam de texto conforme a plataforma.

## 3. Mapa de áreas funcionais

1. Lista de dispositivos Android (fonte USB)
2. Configuração da sessão (resolução/fps/bitrate/codec)
3. Iniciar/parar transmissão + status da sessão
4. Preview ao vivo + tap-to-focus
5. Seletor de modo inteligente
6. Painel de controles de câmera em tempo real
7. Fontes IP/RTSP (CRUD + iniciar/parar)
8. Captura RAW (planejado, ainda sem UI nem comando de backend)
9. Múltiplas fontes simultâneas (parcialmente possível hoje, sem UI dedicada)
10. Banner de erro global
11. Distribuição/instalação (fora do app em si — só onboarding pós-instalação)

---

## 4. Dispositivos Android (fonte USB)

**Já implementado**: `DeviceList.svelte` + comando `list_devices` + eventos
`device_connected` / `device_disconnected` / `device_unauthorized`.

Lista viva (atualiza sozinha, sem polling manual do usuário) de aparelhos
Android detectados por cabo USB. Cada item mostra:

| Campo | Descrição |
|---|---|
| `model` | Nome/modelo do aparelho |
| `serial` | Identificador único (adb) |
| `auth_state` | `authorized` \| `unauthorized` \| `offline` — badge colorido |
| `compatible` | Android < 12 → `false` |
| `incompat_reason` | Texto explicando por que está incompatível |

**Estados visuais**:
- **Vazio**: nenhum aparelho detectado → mensagem orientando conectar o cabo
  com depuração USB habilitada.
- **Não autorizado**: aparelho aparece na lista, mas com um guia passo a
  passo inline (4 passos: olhar o popup no celular → marcar "sempre
  permitir" → tocar permitir → desconectar/reconectar se o popup não
  aparecer). Item não é clicável.
- **Incompatível** (Android < 12): item não clicável, com o motivo em texto.
- **Autorizado + compatível**: clicável, seleciona o aparelho pra iniciar a
  transmissão (não inicia sozinho — é só a fonte escolhida).

**Requisito de tempo**: aparelho precisa aparecer na lista em até 3s após
conectar o cabo (SC-001).

## 5. Configuração da sessão

**Já implementado**: seletores na página principal, desabilitados enquanto a
sessão está ativa (mudar config exige parar e reiniciar).

| Parâmetro | Opções atuais |
|---|---|
| Resolução | 1920×1080 / 1280×720 / 640×480 |
| FPS alvo | 15 / 24 / 30 / 60 (é só o valor pedido ao encoder — ver nota sobre modos inteligentes na seção 7, que pode sobrescrever isso em runtime por câmera real) |
| Bitrate | 1–50 Mbps, numérico |
| Codec | H.264 / H.265 |

Esses 4 campos + o dispositivo selecionado formam o `StreamConfig` que vai
pro comando `start_stream`. Ficam bloqueados durante a sessão ativa — pra
mudar, o usuário para e inicia de novo.

## 6. Iniciar/parar + status da sessão

Botão único que alterna Iniciar/Parar (estados `starting`/`stopping`
desabilitam com texto de loading). Ao lado, dois indicadores textuais:

- **Estado da sessão** (`SessionState`): `idle` \| `starting` \| `streaming`
  \| `stopping` \| `source_lost` \| `reconnecting` \| `{error: "..."}`.
  `source_lost`/`reconnecting` acontecem sozinhos (cabo caiu, celular
  reiniciou) — a UI não pede nada do usuário nesses estados, só informa.
- **Estatísticas ao vivo** (`SessionStats`, só quando a sessão está ativa):
  fps medido em tempo real (contagem real de frames entregues, suavizada por
  EMA — não é o fps "pedido" na configuração) + contador de reconexões desde
  o início da sessão.

## 7. Preview ao vivo + tap-to-focus

**Já implementado**: `Preview.svelte`. Imagem JPEG entregue por evento
(`preview_frame`, ~5 fps, ≤640px de lado — FR-023, propositalmente leve, não
é o stream principal). Estados: placeholder "aguardando stream" (sem sessão),
"aguardando primeiro frame" (sessão criada, ainda sem frame), imagem normal.

**Tap-to-focus**: clicar/tocar no preview envia coordenadas normalizadas
[0,1] pra focar naquela região da câmera real (não é um recorte visual, é
comando de câmera). Cursor deve indicar que a imagem é clicável quando há
sessão ativa.

**Gap conhecido pro design considerar**: o protocolo já emite detecção de
rosto (`faces`, retângulos normalizados) quando o modo Auto/Night está ativo
com foco automático em rosto — hoje esse evento chega no backend mas **não
tem nenhuma representação visual no preview ainda** (nenhum retângulo
desenhado). Se fizer sentido pro produto, é um bom candidato a overlay no
preview (caixas ao redor dos rostos detectados, mesmo estilo de apps de
câmera).

## 8. Modo inteligente

**Já implementado**: `ModeSelector.svelte`. 4 botões (Auto/Night/Sport/Pro),
um ativo por vez, refletindo o estado confirmado pelo evento `control_state`
(não o que foi clicado por último — outro cliente pode ter mudado). Aplicado
sem reabrir a câmera/sem interromper o stream.

| Modo | O que muda na câmera | Uso pretendido |
|---|---|---|
| **Auto** | Foco contínuo, exposição automática, EIS ligada, fps ~30, foco automático em rosto | Videochamadas — default |
| **Night** | fps mais baixo (15–30, o aparelho tem margem pra expor mais), redução de ruído em alta qualidade, +1 EV, EIS ligada, foco automático em rosto | Pouca luz |
| **Sport** | Trava no fps mais alto que o aparelho suportar (idealmente 60), EIS desligada, redução de ruído rápida, sem foco em rosto | Movimento/ação |
| **Pro** | Foco e exposição manuais (libera os controles correspondentes no painel — ver seção 9), sem EIS, sem redução de ruído extra | Controle manual completo |

**Nota importante pro design**: o fps de Night/Sport é o *pedido* — o valor
efetivo é sempre o suportado mais próximo declarado pelo aparelho (nem todo
celular tem 60fps disponível; a UI não deveria prometer "60fps garantido", e
sim algo como "fps mais alto disponível no aparelho").

## 9. Controles de câmera em tempo real

**Já implementado**: `CameraControls.svelte`. Cada controle só aparece
habilitado se a capability correspondente existir em `DeviceCapabilities`
(consultada uma vez por sessão via `get_capabilities`) — princípio da seção
2, nunca falha silenciosa.

| Controle | Tipo de UI | Faixa/opções | Condição de habilitação |
|---|---|---|---|
| Zoom | Slider contínuo | `zoom_range` do aparelho (ex.: 1.0–5.0x) | Sempre (todo aparelho tem zoom digital) |
| Exposição (EV) | Slider | `exposure_comp_range`, tipicamente -2 a +2 | Sempre |
| Balanço de branco | Select | `wb_modes`: auto/luz do dia/nublado/fluorescente/incandescente | Desabilitado se só 1 modo disponível |
| ISO manual | Numérico | `iso_range` (ex.: 50–3200) | **Só habilitado no modo Pro** — em outros modos aparece visível mas desabilitado, com tooltip explicando |
| EIS (estabilização) | Toggle | on/off | Só se `supports_eis` |
| Lanterna (torch) | Toggle | on/off | Só se `supports_torch` |
| Frontal/Traseira | Botão de ação | — | Só se o aparelho tiver as duas câmeras (`hasBothFacings`) — reinicia a sessão, ≤2s |
| Girar | Botão cíclico (0°→90°→180°→270°→0°) | 4 passos | Sempre — ver nota de plataforma na seção 13 |
| Espelhar | Toggle | on/off | Sempre |
| Foco por toque | Ação no preview (não é um controle do painel) | coordenadas normalizadas | Sempre — ver seção 8 |

Foco também tem um terceiro modo (manual por distância, `FocusMode::manual`)
já modelado no protocolo mas **sem controle de UI ainda** — hoje só
automático contínuo e tap-to-focus estão expostos. Se o design cobrir esse
gap, é um slider de distância análogo ao ISO (provavelmente também
restrito ao modo Pro).

Todos os controles usam o mesmo padrão: mudar o valor dispara
`setControl(sessionId, change)`; enquanto a chamada está em voo, o painel
inteiro fica em estado "busy" (evita comandos empilhados); erro aparece no
banner global, não localizado no controle.

## 10. Fontes IP/RTSP

**Já implementado**: `RtspPanel.svelte`. CRUD simples + iniciar/parar por
fonte, independente da sessão Android (podem coexistir).

- **Adicionar**: formulário com nome, URL (`rtsp://host:porta/caminho`) e
  senha opcional. A senha nunca é salva em texto — vai pro cofre de
  segredos do SO; a URL salva nunca contém credenciais.
- **Lista de fontes cadastradas**: nome, URL (truncada se longa), ícone de
  cadeado quando há credencial guardada no cofre.
- **Por fonte**: botão Iniciar/Parar (estado local, não persiste "estava
  rodando" entre reinícios do app) e Remover (bloqueado enquanto rodando —
  precisa parar antes; remover também limpa a credencial do cofre).
- **Erros de conexão** (URL inválida, host inacessível, credenciais erradas)
  aparecem pelo mesmo banner de erro global, com mensagem específica.

Fonte RTSP ativa também alimenta o mesmo componente de Preview (não tem
preview próprio separado hoje).

## 11. Captura RAW (planejado — sem UI nem comando de backend ainda)

Existe no modelo de dados (`RawCaptureJob`, `RawInfo` em `model.rs`) mas
**nenhum comando Tauri foi implementado ainda** — é a User Story 5 do
produto, ainda não construída. Documentando aqui pra quem for desenhar a
interface já prever o espaço, mesmo sem poder ligar a funcionalidade agora:

- **Snapshot RAW**: botão de ação única → salva um DNG na resolução nativa
  do sensor.
- **Sequência RAW**: início/parada de captura contínua, cadência 1–3 fps
  ajustada dinamicamente pela banda USB disponível (a UI precisaria mostrar
  a cadência efetiva em tempo real, não só a nominal).
- Só aparece se o aparelho reportar `raw` != null em `DeviceCapabilities`
  (sensor size + bytes por frame já vêm nesse struct).
- O stream principal sempre tem prioridade sobre a captura RAW quando a
  banda USB aperta — vale um indicador de "cadência reduzida por banda" se
  isso acontecer.

## 12. Múltiplas fontes simultâneas (parcialmente possível, sem UI dedicada)

O backend já suporta uma sessão Android + N fontes RTSP simultâneas (cada
uma com seu próprio dispositivo virtual, isolamento de falha entre elas) —
mas a interface atual só tem *uma* área de preview e não deixa claro, quando
há mais de uma fonte ativa, qual dispositivo virtual corresponde a qual
fonte. Se o design for além do estado atual, vale prever:

- Preview por fonte ativa (não um preview único que troca de dono).
- Identificação clara do dispositivo virtual de cada fonte (nome que aparece
  no OBS/Chrome), hoje só disponível via `VirtualCamera.label`/`id` no
  retorno de `start_stream`/`start_rtsp`.
- Limite prático de 4 fontes ativas simultâneas (não há esse teto hoje no
  código, é uma decisão de produto a validar).

## 13. Diferenças de comportamento Linux vs Windows relevantes pra UX

A interface é a mesma nos dois sistemas, mas o texto de aviso ao lado de
"Girar"/"Espelhar"/"Frontal-Traseira" muda porque o comportamento real muda:

- **Linux**: girar em qualquer ângulo (inclusive 180°/espelhar) reinicia a
  sessão por baixo, porque o pipeline escreve direto no v4l2loopback sem
  passar pelo desktop. Pode gerar um instante sem frame que, em apps tipo
  Meet, às vezes exige F5 na aba pra "acordar" a câmera de novo — vale um
  aviso permanente perto desses controles no Linux.
- **Windows**: só girar 90°/270° reinicia (troca largura↔altura); espelhar e
  girar 180° aplicam ao vivo, sem interrupção, porque o pipeline passa pelo
  Rust antes de entregar ao filtro DirectShow.

## 14. Estados de sessão e erro — tratamento global

`SessionState` é uma máquina de estados única pra toda sessão (Android ou
RTSP): `idle → starting → streaming ⇄ source_lost ⇄ reconnecting → stopping
→ idle`, ou `{error: "mensagem"}` em qualquer ponto. Nenhum desses estados
exige ação do usuário além de "espera" — reconexão é sempre automática.

**Banner de erro global** (`AppError`): usado por qualquer comando que falhe
(iniciar sessão, trocar controle, adicionar fonte RTSP etc.). Sempre tem:
- `msg`: mensagem do erro, direto ao ponto.
- `action_hint`: sugestão do que fazer (opcional, mas quando existe é o
  texto mais importante da tela pro usuário se recuperar sozinho).

Hoje é um banner único no topo/fim da página, compartilhado por todas as
áreas — o design pode decidir se cada área (RTSP, controles, sessão) merece
seu próprio ponto de erro em vez de um banner global genérico.
 ## 15 . o que projetar para o design
 ao conectar a camera ter opção de esconder a aba e deixar em segundo plano
 ao dar falha ou acontecer algo improvavel mostrar notificações do que está acontecendo.
 seguir um design mais moderno e tecnologico.
## 15. Resumo — o que já tem interface funcional vs. o que falta

| Área | Backend | Frontend |
|---|---|---|
| Dispositivos Android | ✅ | ✅ |
| Config de sessão + iniciar/parar | ✅ | ✅ |
| Preview + tap-to-focus | ✅ | ✅ |
| Modo inteligente (Auto/Night/Sport/Pro) | ✅ | ✅ |
| Controles de câmera (zoom/exposição/WB/ISO/EIS/torch/flip/rotate/mirror) | ✅ | ✅ |
| Foco manual por distância | ✅ (protocolo) | ❌ |
| Overlay de rosto detectado no preview | ✅ (evento `faces`) | ❌ |
| Fontes RTSP (CRUD + iniciar/parar) | ✅ | ✅ |
| Captura RAW (snapshot + sequência) | ❌ | ❌ |
| Preview/identificação por fonte quando há múltiplas simultâneas | parcial | ❌ |

Tudo marcado ✅/✅ está funcional mas com UI "de desenvolvedor" (sem
identidade visual, layout utilitário) — é o material primário pra dar forma.
O que tem ❌ no frontend é onde a interface precisa ser desenhada com mais
liberdade, já que não existe nada de referência pra seguir.
